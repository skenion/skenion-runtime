#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
publisher="${repo_root}/scripts/publish-runtime-asset-s3.sh"
existing_checker="${repo_root}/scripts/check-runtime-asset-s3-existing.sh"
tmp_root="$(mktemp -d)"
target="x86_64-unknown-linux-gnu"
version="1.2.3"
release_tag="v1.2.3"
source_commit="1111111111111111111111111111111111111111"
bucket="skenion"
prefix="releases"
public_base="https://cdn.example.test/skenion/releases"

cleanup() {
  if [[ "${KEEP_RUNTIME_PUBLISH_TEST_TMP:-}" == "1" ]]; then
    echo "keeping Runtime publisher validation tmp dir: ${tmp_root}" >&2
  else
    rm -rf "${tmp_root}"
  fi
}
trap cleanup EXIT

fail() {
  echo "$1" >&2
  exit 1
}

sha256_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${path}" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${path}" | awk '{print $1}'
  else
    fail "no sha256 checksum tool found"
  fi
}

write_checksum() {
  local asset="$1"
  local output="$2"

  (
    cd "$(dirname "${asset}")"
    printf '%s  %s\n' "$(sha256_file "$(basename "${asset}")")" "$(basename "${asset}")" >"${output}"
  )
}

metadata_path_for() {
  local object_path="$1"
  printf '%s.stub-metadata' "${object_path}"
}

write_metadata() {
  local object_path="$1"
  local sha="$2"

  cat >"$(metadata_path_for "${object_path}")" <<EOF
sha256=${sha}
component=skenion-runtime
target=${target}
runtime-version=${version}
source-tag=${release_tag}
source-commit=${source_commit}
EOF
}

install_stubs() {
  local bin_dir="$1"
  mkdir -p "${bin_dir}"

  cat >"${bin_dir}/aws" <<'BASH'
#!/usr/bin/env bash
set -euo pipefail

log="${STUB_AWS_LOG:?}"
root="${STUB_S3_ROOT:?}"

size_of() {
  wc -c <"$1" | tr -d '[:space:]'
}

metadata_json_of() {
  local path="$1"
  python3 - "${path}.stub-metadata" <<'PY'
import json
import sys
from pathlib import Path

metadata_path = Path(sys.argv[1])
metadata = {}
if metadata_path.is_file():
    for line in metadata_path.read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            metadata[key] = value
print(json.dumps(metadata, sort_keys=True))
PY
}

write_metadata_file() {
  local path="$1"
  local metadata="$2"

  : >"${path}.stub-metadata"
  IFS=',' read -r -a pairs <<<"${metadata}"
  for pair in "${pairs[@]}"; do
    if [[ "${pair}" == *=* ]]; then
      printf '%s\n' "${pair}" >>"${path}.stub-metadata"
    fi
  done
}

if [[ "${1:-}" == "configure" ]]; then
  echo "configure $*" >>"${log}"
  exit 0
fi

if [[ "${1:-}" == "--endpoint-url" ]]; then
  shift 2
fi

command_name="${1:-}"
shift || true

case "${command_name}" in
  s3api)
    subcommand="${1:-}"
    shift || true
    case "${subcommand}" in
      head-object)
        bucket=""
        key=""
        while [[ $# -gt 0 ]]; do
          case "$1" in
            --bucket)
              bucket="$2"
              shift 2
              ;;
            --key)
              key="$2"
              shift 2
              ;;
            *)
              shift
              ;;
          esac
        done

        echo "head ${bucket}/${key}" >>"${log}"
        path="${root}/${bucket}/${key}"
        if [[ ! -f "${path}" ]]; then
          echo "An error occurred (404) when calling the HeadObject operation: Not Found" >&2
          exit 255
        fi

        printf '{"ContentLength":%s,"Metadata":%s}\n' "$(size_of "${path}")" "$(metadata_json_of "${path}")"
        ;;
      put-object)
        bucket=""
        key=""
        body=""
        metadata=""
        if_none_match=""
        while [[ $# -gt 0 ]]; do
          case "$1" in
            --bucket)
              bucket="$2"
              shift 2
              ;;
            --key)
              key="$2"
              shift 2
              ;;
            --body)
              body="$2"
              shift 2
              ;;
            --metadata)
              metadata="$2"
              shift 2
              ;;
            --content-type)
              shift 2
              ;;
            --if-none-match)
              if_none_match="$2"
              shift 2
              ;;
            *)
              shift
              ;;
          esac
        done

        if [[ "${if_none_match}" != "*" ]]; then
          echo "put-object missing required --if-none-match '*'" >&2
          exit 2
        fi

        path="${root}/${bucket}/${key}"
        mkdir -p "$(dirname "${path}")"
        if [[ "${STUB_AWS_CONCURRENT_CREATE_ON_PUT:-}" == "1" && ! -f "${path}" ]]; then
          printf 'concurrent object\n' >"${path}"
          cat >"${path}.stub-metadata" <<EOF
sha256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
component=skenion-runtime
target=x86_64-unknown-linux-gnu
runtime-version=1.2.3
source-tag=v1.2.3
source-commit=1111111111111111111111111111111111111111
EOF
        fi
        if [[ -f "${path}" ]]; then
          echo "An error occurred (PreconditionFailed) when calling the PutObject operation: At least one of the pre-conditions you specified did not hold" >&2
          echo "put-precondition-failed ${bucket}/${key}" >>"${log}"
          exit 255
        fi

        command cp "${body}" "${path}"
        rm -f "${path}.stub-metadata"
        if [[ "${STUB_AWS_DROP_PUT_METADATA:-}" != "1" ]]; then
          write_metadata_file "${path}" "${metadata}"
        fi
        echo "put ${bucket}/${key}" >>"${log}"
        printf '{"ETag":"stub-etag"}\n'
        ;;
      *)
        echo "unsupported aws s3api subcommand: ${subcommand}" >&2
        exit 2
        ;;
    esac
    ;;
  s3)
    subcommand="${1:-}"
    shift || true
    if [[ "${subcommand}" == "cp" && "${1:-}" == s3://* ]]; then
      echo "unexpected authenticated S3 object download in metadata-only publisher" >&2
      echo "unexpected-get ${1#s3://}" >>"${log}"
      exit 2
    fi
    echo "unsupported aws s3 command in metadata-only publisher" >&2
    exit 2
    ;;
  *)
    echo "unsupported aws command: ${command_name}" >&2
    exit 2
    ;;
esac
BASH

  cat >"${bin_dir}/curl" <<'BASH'
#!/usr/bin/env bash
set -euo pipefail

log="${STUB_CURL_LOG:?}"
public_base="${STUB_PUBLIC_BASE_URL:?}"
public_root="${STUB_PUBLIC_ROOT:?}"
state_root="${STUB_CURL_STATE_DIR:?}"
method="GET"
dump_header=""
url=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --head|-I)
      method="HEAD"
      shift
      ;;
    --dump-header|-D)
      dump_header="$2"
      shift 2
      ;;
    --output|-o)
      shift 2
      ;;
    --fail|--silent|--show-error|--location|-f|-s|-S|-L)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

if [[ "${method}" != "HEAD" ]]; then
  echo "unexpected public body download in metadata-only publisher" >&2
  exit 2
fi
if [[ "${url}" != "${public_base}/"* ]]; then
  echo "unexpected public URL: ${url}" >&2
  exit 2
fi

relative_key="${url#"${public_base}/"}"
path="${public_root}/${relative_key}"
if [[ ! -f "${path}" ]]; then
  echo "curl: (22) The requested URL returned error: 404" >&2
  exit 22
fi

echo "${method} ${relative_key}" >>"${log}"
size="$(wc -c <"${path}" | tr -d '[:space:]')"
mkdir -p "${state_root}"
counter_name="$(printf '%s' "${method}_${relative_key}" | tr -c '[:alnum:]' '_')"
counter_path="${state_root}/${counter_name}.count"
attempt=0
if [[ -f "${counter_path}" ]]; then
  attempt="$(sed -n '1p' "${counter_path}")"
fi
attempt=$((attempt + 1))
printf '%s\n' "${attempt}" >"${counter_path}"

if [[ "${attempt}" -le "${STUB_CURL_FAIL_HEAD_ATTEMPTS:-0}" ]]; then
  echo "curl: (22) The requested URL returned error: 403" >&2
  exit 22
fi

metadata_path="${path}.stub-metadata"
sha=""
if [[ -f "${metadata_path}" && "${STUB_CURL_DROP_SHA256_METADATA:-}" != "1" ]]; then
  sha="$(awk -F= '$1 == "sha256" { print $2; exit }' "${metadata_path}")"
fi
if [[ "${STUB_CURL_CORRUPT_SHA256_METADATA:-}" == "1" ]]; then
  sha="0000000000000000000000000000000000000000000000000000000000000000"
fi

{
  printf 'HTTP/1.1 200 OK\r\n'
  printf 'Content-Length: %s\r\n' "${size}"
  if [[ -n "${sha}" ]]; then
    printf 'x-amz-meta-sha256: %s\r\n' "${sha}"
  fi
  printf '\r\n'
} >"${dump_header}"
BASH

  chmod +x "${bin_dir}/aws" "${bin_dir}/curl"
}

prepare_case() {
  local case_dir="$1"
  local content="$2"
  local asset_dir="${case_dir}/dist"
  local asset_path="${asset_dir}/skenion-runtime-v${version}-${target}.tar.gz"

  mkdir -p "${asset_dir}"
  printf '%s\n' "${content}" >"${asset_path}"
  write_checksum "${asset_path}" "${asset_path}.sha256"
}

asset_path_for() {
  local case_dir="$1"
  printf '%s/dist/skenion-runtime-v%s-%s.tar.gz' "${case_dir}" "${version}" "${target}"
}

object_path_for_key() {
  local case_dir="$1"
  local key="$2"
  printf '%s/s3/%s/%s' "${case_dir}" "${bucket}" "${key}"
}

runtime_key_for_asset() {
  local asset="$1"
  printf '%s/skenion-runtime/%s/%s/%s' "${prefix}" "${release_tag}" "${target}" "$(basename "${asset}")"
}

reset_logs() {
  local case_dir="$1"
  mkdir -p "${case_dir}/s3/${bucket}/${prefix}"
  : >"${case_dir}/aws.log"
  : >"${case_dir}/curl.log"
}

base_env_args_for() {
  local case_dir="$1"

  base_env_args=(
    "PATH=${tmp_root}/bin:${PATH}" \
    "STUB_AWS_LOG=${case_dir}/aws.log" \
    "STUB_CURL_LOG=${case_dir}/curl.log" \
    "STUB_S3_ROOT=${case_dir}/s3" \
    "STUB_CURL_STATE_DIR=${case_dir}/curl-state" \
    "STUB_PUBLIC_BASE_URL=${public_base}" \
    "STUB_PUBLIC_ROOT=${case_dir}/s3/${bucket}/${prefix}" \
    "GITHUB_ACTIONS=true" \
    "GITHUB_EVENT_NAME=workflow_dispatch" \
    "SKENION_RELEASE_S3_ENDPOINT=https://s3.example.test" \
    "SKENION_RELEASE_S3_REGION=us-east-1" \
    "SKENION_RELEASE_S3_BUCKET=${bucket}" \
    "SKENION_RELEASE_S3_PREFIX=${prefix}" \
    "SKENION_RELEASE_S3_ACCESS_KEY_ID=test-access-key" \
    "SKENION_RELEASE_S3_SECRET_ACCESS_KEY=test-secret-key" \
    "SKENION_RELEASE_S3_FORCE_PATH_STYLE=true" \
    "SKENION_RELEASE_PUBLIC_BASE_URL=${public_base}" \
    "SKENION_PUBLIC_VERIFY_ATTEMPTS=3" \
    "SKENION_PUBLIC_VERIFY_SLEEP_SECONDS=0" \
    "SOURCE_COMMIT=${source_commit}" \
    "RELEASE_TIER=release-blocking" \
    "CONTRACTS_VERSION=1.2.0" \
    "CONTRACTS_LINE=1.2"
  )
}

run_publisher() {
  local case_dir="$1"
  local skip_public=false
  local asset_path
  local checksum_path
  local -a base_env_args
  local -a env_args=()
  local -a publisher_args=()
  shift

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --skip-public-verification)
        skip_public=true
        shift
        ;;
      *=*)
        env_args+=("$1")
        shift
        ;;
      *)
        fail "unknown run_publisher argument: $1"
        ;;
    esac
  done

  if [[ "${skip_public}" == "true" ]]; then
    publisher_args+=(--skip-public-verification)
  fi

  reset_logs "${case_dir}"
  base_env_args_for "${case_dir}"
  asset_path="$(asset_path_for "${case_dir}")"
  checksum_path="${asset_path}.sha256"

  env "${base_env_args[@]}" \
    "${publisher}" \
    --dry-run \
    "${target}" \
    "${version}" \
    "${release_tag}" \
    "${asset_path}" \
    "${checksum_path}" >/dev/null

  env "${base_env_args[@]}" \
    "${env_args[@]+"${env_args[@]}"}" \
    "${publisher}" \
    --use-existing-manifest \
    "${publisher_args[@]+"${publisher_args[@]}"}" \
    "${target}" \
    "${version}" \
    "${release_tag}" \
    "${asset_path}" \
    "${checksum_path}"
}

run_existing_checker() {
  local case_dir="$1"
  local -a base_env_args
  shift

  reset_logs "${case_dir}"
  base_env_args_for "${case_dir}"
  env "${base_env_args[@]}" \
    "$@" \
    "${existing_checker}" \
    "${target}" \
    "${version}" \
    "${release_tag}"
}

expect_success() {
  local case_dir="$1"
  shift

  if ! run_publisher "${case_dir}" "$@" >"${case_dir}/output.log" 2>&1; then
    sed 's/^/[publisher] /' "${case_dir}/output.log" >&2
    fail "expected publisher case to succeed: $(basename "${case_dir}")"
  fi
}

expect_failure() {
  local case_dir="$1"
  shift

  if run_publisher "${case_dir}" "$@" >"${case_dir}/output.log" 2>&1; then
    sed 's/^/[publisher] /' "${case_dir}/output.log" >&2
    fail "expected publisher case to fail: $(basename "${case_dir}")"
  fi
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  if ! grep -Eq "${pattern}" "${file}"; then
    sed 's/^/[file] /' "${file}" >&2
    fail "expected ${file} to contain pattern: ${pattern}"
  fi
}

assert_not_contains() {
  local file="$1"
  local pattern="$2"
  if grep -Eq "${pattern}" "${file}"; then
    sed 's/^/[file] /' "${file}" >&2
    fail "expected ${file} not to contain pattern: ${pattern}"
  fi
}

assert_no_body_downloads() {
  local case_dir="$1"

  assert_not_contains "${case_dir}/aws.log" '^unexpected-get '
  assert_not_contains "${case_dir}/curl.log" '^GET '
}

assert_put_count() {
  local case_dir="$1"
  local expected="$2"
  local actual

  actual="$(grep -c '^put ' "${case_dir}/aws.log" || true)"
  if [[ "${actual}" != "${expected}" ]]; then
    sed 's/^/[aws] /' "${case_dir}/aws.log" >&2
    fail "expected ${expected} put events, saw ${actual}"
  fi
}

assert_no_head_after_put_for_same_key() {
  local case_dir="$1"

  python3 - "${case_dir}/aws.log" <<'PY'
import sys
from pathlib import Path

events = [line.strip() for line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines() if line.strip()]
for index, event in enumerate(events):
    if not event.startswith("put "):
        continue
    key = event.removeprefix("put ")
    later_head = f"head {key}"
    if later_head in events[index + 1 :]:
        print(f"post-upload HEAD detected for {key}", file=sys.stderr)
        raise SystemExit(1)
PY
}

seed_existing_release_objects() {
  local case_dir="$1"
  local asset
  local checksum
  local manifest
  local asset_key
  local checksum_key
  local manifest_key
  local asset_object
  local checksum_object
  local manifest_object

  prepare_case "${case_dir}" "runtime existing release artifact"
  asset="$(asset_path_for "${case_dir}")"
  checksum="${asset}.sha256"
  manifest="${asset}.manifest.json"
  cat >"${manifest}" <<EOF
{"runtimeVersion":"${version}","target":"${target}"}
EOF

  asset_key="$(runtime_key_for_asset "${asset}")"
  checksum_key="${asset_key}.sha256"
  manifest_key="${asset_key}.manifest.json"
  asset_object="$(object_path_for_key "${case_dir}" "${asset_key}")"
  checksum_object="$(object_path_for_key "${case_dir}" "${checksum_key}")"
  manifest_object="$(object_path_for_key "${case_dir}" "${manifest_key}")"

  mkdir -p "$(dirname "${asset_object}")"
  command cp "${asset}" "${asset_object}"
  command cp "${checksum}" "${checksum_object}"
  command cp "${manifest}" "${manifest_object}"
  write_metadata "${asset_object}" "$(sha256_file "${asset_object}")"
  write_metadata "${checksum_object}" "$(sha256_file "${checksum_object}")"
  write_metadata "${manifest_object}" "$(sha256_file "${manifest_object}")"
}

assert_github_actions_guard_case() {
  local case_dir="${tmp_root}/github-actions-guard"

  prepare_case "${case_dir}" "runtime github actions guard artifact"
  expect_failure "${case_dir}" GITHUB_ACTIONS= GITHUB_EVENT_NAME=push
  assert_contains "${case_dir}/output.log" 'Runtime release artifact publishing must run from GitHub Actions'
  if [[ -s "${case_dir}/aws.log" ]]; then
    sed 's/^/[aws] /' "${case_dir}/aws.log" >&2
    fail "publisher reached S3 stub despite GitHub Actions guard refusal"
  fi
}

assert_success_case() {
  local case_dir="${tmp_root}/success"
  local manifest

  prepare_case "${case_dir}" "runtime success artifact"
  expect_success "${case_dir}"
  manifest="$(asset_path_for "${case_dir}").manifest.json"
  assert_no_body_downloads "${case_dir}"
  assert_put_count "${case_dir}" 3
  assert_no_head_after_put_for_same_key "${case_dir}"

  python3 - "${manifest}" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    manifest = json.load(fh)

assert manifest["schema"] == "skenion.runtime.releaseArtifact.v1"
assert manifest["component"] == "skenion-runtime"
assert manifest["runtimeVersion"] == "1.2.3"
assert manifest["releaseTag"] == "v1.2.3"
assert manifest["target"] == "x86_64-unknown-linux-gnu"
assert manifest["artifact"]["s3"]["bucket"] == "skenion"
assert manifest["artifact"]["publicUrl"].startswith("https://cdn.example.test/skenion/releases/")
assert manifest["checksum"]["publicUrl"].endswith(".sha256")
assert manifest["manifest"]["publicUrl"].endswith(".manifest.json")
PY

  assert_contains "${case_dir}/curl.log" '^HEAD skenion-runtime/v1\.2\.3/x86_64-unknown-linux-gnu/.*\.tar\.gz$'
  assert_contains "${case_dir}/curl.log" '^HEAD skenion-runtime/v1\.2\.3/x86_64-unknown-linux-gnu/.*\.sha256$'
  assert_contains "${case_dir}/curl.log" '^HEAD skenion-runtime/v1\.2\.3/x86_64-unknown-linux-gnu/.*\.manifest\.json$'
}

assert_upload_missing_s3_metadata_is_not_a_failure_case() {
  local case_dir="${tmp_root}/upload-missing-s3-metadata"

  prepare_case "${case_dir}" "runtime upload missing s3 metadata artifact"
  expect_success "${case_dir}" --skip-public-verification STUB_AWS_DROP_PUT_METADATA=1
  assert_no_body_downloads "${case_dir}"
  assert_put_count "${case_dir}" 3
  assert_no_head_after_put_for_same_key "${case_dir}"
  assert_not_contains "${case_dir}/output.log" 'S3 metadata does not match expected immutable artifact'
  assert_contains "${case_dir}/output.log" 'uploaded Runtime release object: s3://skenion/releases/skenion-runtime/v1\.2\.3/x86_64-unknown-linux-gnu/.*\.tar\.gz'
}

assert_public_head_retry_case() {
  local case_dir="${tmp_root}/public-head-retry"
  local head_count

  prepare_case "${case_dir}" "runtime public head retry artifact"
  expect_success "${case_dir}" STUB_CURL_FAIL_HEAD_ATTEMPTS=2
  assert_no_body_downloads "${case_dir}"
  assert_contains "${case_dir}/output.log" 'public Runtime release asset .* is not ready on attempt 1/3: HEAD request failed; retrying in 0s'
  assert_contains "${case_dir}/output.log" 'public Runtime release asset .* is not ready on attempt 2/3: HEAD request failed; retrying in 0s'
  head_count="$(grep -c '^HEAD skenion-runtime/v1\.2\.3/x86_64-unknown-linux-gnu/.*\.tar\.gz$' "${case_dir}/curl.log" || true)"
  if [[ "${head_count}" != "3" ]]; then
    sed 's/^/[curl] /' "${case_dir}/curl.log" >&2
    fail "expected public asset HEAD to be retried until third attempt, saw ${head_count}"
  fi
}

assert_public_missing_metadata_failure_case() {
  local case_dir="${tmp_root}/public-missing-metadata"

  prepare_case "${case_dir}" "runtime public missing metadata artifact"
  expect_failure "${case_dir}" STUB_CURL_DROP_SHA256_METADATA=1
  assert_no_body_downloads "${case_dir}"
  assert_contains "${case_dir}/output.log" 'missing x-amz-meta-sha256'
}

assert_public_mismatched_metadata_failure_case() {
  local case_dir="${tmp_root}/public-mismatched-metadata"

  prepare_case "${case_dir}" "runtime public mismatched metadata artifact"
  expect_failure "${case_dir}" STUB_CURL_CORRUPT_SHA256_METADATA=1
  assert_no_body_downloads "${case_dir}"
  assert_contains "${case_dir}/output.log" 'public Runtime release .* HEAD metadata does not match local immutable artifact'
}

assert_existing_matching_metadata_skips_upload_case() {
  local case_dir="${tmp_root}/existing-matching-metadata"
  local matching_count

  prepare_case "${case_dir}" "runtime existing matching metadata artifact"
  expect_success "${case_dir}" --skip-public-verification
  reset_logs "${case_dir}"
  expect_success "${case_dir}" --skip-public-verification
  assert_no_body_downloads "${case_dir}"
  matching_count="$(grep -c 'object already exists and will not be overwritten' "${case_dir}/output.log" || true)"
  if [[ "${matching_count}" != "3" ]]; then
    sed 's/^/[publisher] /' "${case_dir}/output.log" >&2
    fail "expected all three existing release objects to match; saw ${matching_count}"
  fi
  assert_put_count "${case_dir}" 0
}

assert_existing_missing_metadata_skips_matching_asset_case() {
  local case_dir="${tmp_root}/existing-missing-metadata"
  local asset
  local asset_key
  local existing

  prepare_case "${case_dir}" "runtime existing missing metadata artifact"
  asset="$(asset_path_for "${case_dir}")"
  asset_key="$(runtime_key_for_asset "${asset}")"
  existing="$(object_path_for_key "${case_dir}" "${asset_key}")"
  mkdir -p "$(dirname "${existing}")"
  command cp "${asset}" "${existing}"

  expect_success "${case_dir}" --skip-public-verification
  assert_no_body_downloads "${case_dir}"
  assert_contains "${case_dir}/output.log" 'object already exists without immutable metadata and matching size'
  assert_contains "${case_dir}/output.log" 'object already exists and will not be overwritten'
  assert_not_contains "${case_dir}/aws.log" "^put ${bucket}/${asset_key}$"
  assert_contains "${case_dir}/aws.log" "^put ${bucket}/${asset_key}\\.sha256$"
  assert_contains "${case_dir}/aws.log" "^put ${bucket}/${asset_key}\\.manifest\\.json$"
  assert_no_head_after_put_for_same_key "${case_dir}"
}

assert_existing_mismatched_metadata_fails_case() {
  local case_dir="${tmp_root}/existing-mismatched-metadata"
  local asset
  local asset_key
  local existing

  prepare_case "${case_dir}" "runtime existing mismatched metadata artifact"
  asset="$(asset_path_for "${case_dir}")"
  asset_key="$(runtime_key_for_asset "${asset}")"
  existing="$(object_path_for_key "${case_dir}" "${asset_key}")"
  mkdir -p "$(dirname "${existing}")"
  command cp "${asset}" "${existing}"
  write_metadata "${existing}" "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"

  expect_failure "${case_dir}" --skip-public-verification
  assert_no_body_downloads "${case_dir}"
  assert_contains "${case_dir}/output.log" 'S3 metadata does not match expected immutable artifact'
  assert_contains "${case_dir}/output.log" 'refusing to overwrite existing Runtime release artifact'
  assert_not_contains "${case_dir}/aws.log" "^put ${bucket}/${asset_key}$"
}

assert_head_miss_concurrent_put_race_case() {
  local case_dir="${tmp_root}/head-miss-concurrent-put"
  local asset
  local asset_name
  local raced_path

  prepare_case "${case_dir}" "runtime concurrent put race artifact"
  asset="$(asset_path_for "${case_dir}")"
  asset_name="$(basename "${asset}")"
  raced_path="${case_dir}/s3/${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}"

  expect_failure "${case_dir}" --skip-public-verification STUB_AWS_CONCURRENT_CREATE_ON_PUT=1
  assert_no_body_downloads "${case_dir}"
  assert_contains "${case_dir}/output.log" 'failed to conditionally upload Runtime release artifact without overwriting'
  assert_contains "${case_dir}/output.log" 'PreconditionFailed'
  assert_contains "${case_dir}/aws.log" "^put-precondition-failed ${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}$"
  assert_not_contains "${case_dir}/aws.log" "^put ${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}$"
  assert_contains "${raced_path}" '^concurrent object$'
}

assert_existing_checker_missing_objects_case() {
  local case_dir="${tmp_root}/existing-checker-missing"

  mkdir -p "${case_dir}"
  run_existing_checker "${case_dir}" >"${case_dir}/output.log" 2>&1
  assert_contains "${case_dir}/output.log" 'runtime_asset_exists=false'
  assert_put_count "${case_dir}" 0
  if [[ -s "${case_dir}/curl.log" ]]; then
    sed 's/^/[curl] /' "${case_dir}/curl.log" >&2
    fail "existing checker touched public CDN despite S3-only preflight"
  fi
}

assert_existing_checker_valid_objects_case() {
  local case_dir="${tmp_root}/existing-checker-valid"

  mkdir -p "${case_dir}"
  seed_existing_release_objects "${case_dir}"
  run_existing_checker "${case_dir}" >"${case_dir}/output.log" 2>&1
  assert_contains "${case_dir}/output.log" 'runtime_asset_exists=true'
  assert_put_count "${case_dir}" 0
  if [[ -s "${case_dir}/curl.log" ]]; then
    sed 's/^/[curl] /' "${case_dir}/curl.log" >&2
    fail "existing checker touched public CDN despite S3-only preflight"
  fi
}

assert_existing_checker_metadata_free_objects_case() {
  local case_dir="${tmp_root}/existing-checker-metadata-free"
  local asset
  local asset_key
  local asset_object

  mkdir -p "${case_dir}"
  seed_existing_release_objects "${case_dir}"
  asset="$(asset_path_for "${case_dir}")"
  asset_key="$(runtime_key_for_asset "${asset}")"
  asset_object="$(object_path_for_key "${case_dir}" "${asset_key}")"
  : >"$(metadata_path_for "${asset_object}")"

  run_existing_checker "${case_dir}" >"${case_dir}/output.log" 2>&1
  assert_contains "${case_dir}/output.log" 'runtime_asset_exists=true'
  assert_contains "${case_dir}/output.log" 'found existing Runtime release asset without S3 metadata'
  assert_put_count "${case_dir}" 0
  if [[ -s "${case_dir}/curl.log" ]]; then
    sed 's/^/[curl] /' "${case_dir}/curl.log" >&2
    fail "existing checker touched public CDN despite metadata-free existing object"
  fi
}

install_stubs "${tmp_root}/bin"
assert_github_actions_guard_case
assert_success_case
assert_upload_missing_s3_metadata_is_not_a_failure_case
assert_public_head_retry_case
assert_public_missing_metadata_failure_case
assert_public_mismatched_metadata_failure_case
assert_existing_matching_metadata_skips_upload_case
assert_existing_missing_metadata_skips_matching_asset_case
assert_existing_mismatched_metadata_fails_case
assert_head_miss_concurrent_put_race_case
assert_existing_checker_missing_objects_case
assert_existing_checker_valid_objects_case
assert_existing_checker_metadata_free_objects_case

echo "Runtime DSUB S3 publisher validation passed."
