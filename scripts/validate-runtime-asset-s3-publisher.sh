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
  rm -rf "${tmp_root}"
}
trap cleanup EXIT

sha256_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${path}" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${path}" | awk '{print $1}'
  else
    echo "no sha256 checksum tool found" >&2
    exit 1
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
        write_metadata_file "${path}" "${metadata}"
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

run_publisher() {
  local case_dir="$1"
  local asset_path
  local checksum_path
  local -a base_env
  shift

  mkdir -p "${case_dir}/s3/${bucket}/${prefix}"
  : >"${case_dir}/aws.log"
  : >"${case_dir}/curl.log"
  asset_path="$(asset_path_for "${case_dir}")"
  checksum_path="${asset_path}.sha256"

  base_env=(
    "PATH=${tmp_root}/bin:${PATH}"
    "STUB_AWS_LOG=${case_dir}/aws.log"
    "STUB_CURL_LOG=${case_dir}/curl.log"
    "STUB_S3_ROOT=${case_dir}/s3"
    "STUB_CURL_STATE_DIR=${case_dir}/curl-state"
    "STUB_PUBLIC_BASE_URL=${public_base}"
    "STUB_PUBLIC_ROOT=${case_dir}/s3/${bucket}/${prefix}"
    "GITHUB_ACTIONS=true"
    "GITHUB_EVENT_NAME=workflow_dispatch"
    "SKENION_RELEASE_S3_ENDPOINT=https://s3.example.test"
    "SKENION_RELEASE_S3_REGION=us-east-1"
    "SKENION_RELEASE_S3_BUCKET=${bucket}"
    "SKENION_RELEASE_S3_PREFIX=${prefix}"
    "SKENION_RELEASE_S3_ACCESS_KEY_ID=test-access-key"
    "SKENION_RELEASE_S3_SECRET_ACCESS_KEY=test-secret-key"
    "SKENION_RELEASE_S3_FORCE_PATH_STYLE=true"
    "SKENION_RELEASE_PUBLIC_BASE_URL=${public_base}"
    "SKENION_PUBLIC_VERIFY_ATTEMPTS=3"
    "SKENION_PUBLIC_VERIFY_SLEEP_SECONDS=0"
    "SOURCE_COMMIT=${source_commit}"
    "RELEASE_TIER=release-blocking"
    "CONTRACTS_VERSION=1.2.0"
    "CONTRACTS_LINE=1.2"
  )

  env "${base_env[@]}" \
    "${publisher}" \
    --dry-run \
    "${target}" \
    "${version}" \
    "${release_tag}" \
    "${asset_path}" \
    "${checksum_path}" >/dev/null

  env "${base_env[@]}" \
    "$@" \
    "${publisher}" \
    --use-existing-manifest \
    "${target}" \
    "${version}" \
    "${release_tag}" \
    "${asset_path}" \
    "${checksum_path}"
}

run_existing_checker() {
  local case_dir="$1"
  local -a base_env
  shift

  mkdir -p "${case_dir}/s3/${bucket}/${prefix}"
  : >"${case_dir}/aws.log"
  : >"${case_dir}/curl.log"

  base_env=(
    "PATH=${tmp_root}/bin:${PATH}"
    "STUB_AWS_LOG=${case_dir}/aws.log"
    "STUB_CURL_LOG=${case_dir}/curl.log"
    "STUB_S3_ROOT=${case_dir}/s3"
    "STUB_CURL_STATE_DIR=${case_dir}/curl-state"
    "STUB_PUBLIC_BASE_URL=${public_base}"
    "STUB_PUBLIC_ROOT=${case_dir}/s3/${bucket}/${prefix}"
    "SKENION_RELEASE_S3_ENDPOINT=https://s3.example.test"
    "SKENION_RELEASE_S3_REGION=us-east-1"
    "SKENION_RELEASE_S3_BUCKET=${bucket}"
    "SKENION_RELEASE_S3_PREFIX=${prefix}"
    "SKENION_RELEASE_S3_ACCESS_KEY_ID=test-access-key"
    "SKENION_RELEASE_S3_SECRET_ACCESS_KEY=test-secret-key"
    "SKENION_RELEASE_S3_FORCE_PATH_STYLE=true"
    "SKENION_RELEASE_PUBLIC_BASE_URL=${public_base}"
    "SOURCE_COMMIT=${source_commit}"
  )

  env "${base_env[@]}" \
    "$@" \
    "${existing_checker}" \
    "${target}" \
    "${version}" \
    "${release_tag}"
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

assert_no_body_downloads() {
  local case_dir="$1"

  if grep -q '^unexpected-get ' "${case_dir}/aws.log"; then
    echo "publisher performed authenticated S3 object download" >&2
    exit 1
  fi
  if grep -q '^GET ' "${case_dir}/curl.log"; then
    echo "publisher performed public body download" >&2
    exit 1
  fi
}

assert_github_actions_guard_case() {
  local case_dir="${tmp_root}/github-actions-guard"

  prepare_case "${case_dir}" "runtime github actions guard artifact"
  if run_publisher "${case_dir}" GITHUB_ACTIONS= GITHUB_EVENT_NAME=push >"${case_dir}/output.log" 2>&1; then
    echo "expected GitHub Actions guard publisher case to fail" >&2
    exit 1
  fi

  grep -q 'Runtime release artifact publishing must run from GitHub Actions' "${case_dir}/output.log"
  if [[ -s "${case_dir}/aws.log" ]]; then
    echo "publisher reached S3 stub despite GitHub Actions guard refusal" >&2
    exit 1
  fi
}

assert_success_case() {
  local case_dir="${tmp_root}/success"
  local manifest

  prepare_case "${case_dir}" "runtime success artifact"
  run_publisher "${case_dir}" >"${case_dir}/output.log" 2>&1
  manifest="$(asset_path_for "${case_dir}").manifest.json"
  assert_no_body_downloads "${case_dir}"

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

  python3 - "${case_dir}/aws.log" <<'PY'
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    events = [line.strip() for line in fh if line.strip()]

put_indexes = [
    (index, event.removeprefix("put "))
    for index, event in enumerate(events)
    if event.startswith("put ")
]
assert len(put_indexes) == 3, events
for index, key in put_indexes:
    assert any(event == f"head {key}" for event in events[index + 1 :]), (key, events)
PY

  grep -q '^HEAD skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.tar\.gz$' "${case_dir}/curl.log"
  grep -q '^HEAD skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.sha256$' "${case_dir}/curl.log"
  grep -q '^HEAD skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.manifest\.json$' "${case_dir}/curl.log"
}

assert_public_head_retry_case() {
  local case_dir="${tmp_root}/public-head-retry"
  local head_count

  prepare_case "${case_dir}" "runtime public head retry artifact"
  run_publisher "${case_dir}" STUB_CURL_FAIL_HEAD_ATTEMPTS=2 >"${case_dir}/output.log" 2>&1
  assert_no_body_downloads "${case_dir}"

  grep -q 'public Runtime release asset .* is not ready on attempt 1/3: HEAD request failed; retrying in 0s' "${case_dir}/output.log"
  grep -q 'public Runtime release asset .* is not ready on attempt 2/3: HEAD request failed; retrying in 0s' "${case_dir}/output.log"
  head_count="$(grep -c '^HEAD skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.tar\.gz$' "${case_dir}/curl.log")"
  if [[ "${head_count}" != "3" ]]; then
    echo "expected public asset HEAD to be retried until third attempt, saw ${head_count}" >&2
    exit 1
  fi
}

assert_public_missing_metadata_failure_case() {
  local case_dir="${tmp_root}/public-missing-metadata"

  prepare_case "${case_dir}" "runtime public missing metadata artifact"
  if run_publisher "${case_dir}" STUB_CURL_DROP_SHA256_METADATA=1 >"${case_dir}/output.log" 2>&1; then
    echo "expected public missing metadata case to fail" >&2
    exit 1
  fi
  assert_no_body_downloads "${case_dir}"

  grep -q 'missing x-amz-meta-sha256' "${case_dir}/output.log"
}

assert_public_mismatched_metadata_failure_case() {
  local case_dir="${tmp_root}/public-mismatched-metadata"

  prepare_case "${case_dir}" "runtime public mismatched metadata artifact"
  if run_publisher "${case_dir}" STUB_CURL_CORRUPT_SHA256_METADATA=1 >"${case_dir}/output.log" 2>&1; then
    echo "expected public mismatched metadata case to fail" >&2
    exit 1
  fi
  assert_no_body_downloads "${case_dir}"

  grep -q 'public Runtime release .* HEAD metadata does not match local immutable artifact' "${case_dir}/output.log"
}

assert_existing_matching_metadata_skips_upload_case() {
  local case_dir="${tmp_root}/existing-matching-metadata"
  local matching_count

  prepare_case "${case_dir}" "runtime existing matching metadata artifact"
  run_publisher "${case_dir}" >/dev/null 2>&1
  run_publisher "${case_dir}" >"${case_dir}/output.log" 2>&1
  assert_no_body_downloads "${case_dir}"

  grep -q 'object already exists with matching immutable metadata' "${case_dir}/output.log"
  matching_count="$(grep -c 'object already exists with matching immutable metadata' "${case_dir}/output.log")"
  if [[ "${matching_count}" != "3" ]]; then
    echo "expected all three existing release objects to match by metadata; saw ${matching_count}" >&2
    exit 1
  fi
  if grep -q '^put ' "${case_dir}/aws.log"; then
    echo "publisher uploaded despite existing matching metadata for all release objects" >&2
    exit 1
  fi
}

assert_existing_missing_metadata_fails_case() {
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

  if run_publisher "${case_dir}" >"${case_dir}/output.log" 2>&1; then
    echo "expected existing missing metadata case to fail" >&2
    exit 1
  fi
  assert_no_body_downloads "${case_dir}"

  grep -q 'S3 metadata does not match expected immutable artifact' "${case_dir}/output.log"
  grep -q 'refusing to overwrite existing Runtime release artifact' "${case_dir}/output.log"
  if grep -q "^put ${bucket}/${asset_key}$" "${case_dir}/aws.log"; then
    echo "publisher uploaded despite existing missing metadata" >&2
    exit 1
  fi
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

  if run_publisher "${case_dir}" >"${case_dir}/output.log" 2>&1; then
    echo "expected existing mismatched metadata case to fail" >&2
    exit 1
  fi
  assert_no_body_downloads "${case_dir}"

  grep -q 'S3 metadata does not match expected immutable artifact' "${case_dir}/output.log"
  grep -q 'refusing to overwrite existing Runtime release artifact' "${case_dir}/output.log"
  if grep -q "^put ${bucket}/${asset_key}$" "${case_dir}/aws.log"; then
    echo "publisher uploaded despite existing mismatched metadata" >&2
    exit 1
  fi
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

  if run_publisher "${case_dir}" STUB_AWS_CONCURRENT_CREATE_ON_PUT=1 >"${case_dir}/output.log" 2>&1; then
    echo "expected concurrent conditional put race case to fail" >&2
    exit 1
  fi
  assert_no_body_downloads "${case_dir}"

  grep -q 'failed to conditionally upload Runtime release artifact without overwriting' "${case_dir}/output.log"
  grep -q 'PreconditionFailed' "${case_dir}/output.log"
  grep -q "^put-precondition-failed ${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}$" "${case_dir}/aws.log"
  if grep -q "^put ${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}$" "${case_dir}/aws.log"; then
    echo "publisher overwrote concurrent object despite conditional put failure" >&2
    exit 1
  fi
  grep -q '^concurrent object$' "${raced_path}"
}

assert_existing_checker_missing_objects_case() {
  local case_dir="${tmp_root}/existing-checker-missing"

  mkdir -p "${case_dir}"
  run_existing_checker "${case_dir}" >"${case_dir}/output.log" 2>&1
  grep -q 'runtime_asset_exists=false' "${case_dir}/output.log"
  if grep -q '^put ' "${case_dir}/aws.log"; then
    echo "existing checker uploaded despite missing release objects" >&2
    exit 1
  fi
  if [[ -s "${case_dir}/curl.log" ]]; then
    echo "existing checker touched public CDN despite S3-only preflight" >&2
    exit 1
  fi
}

assert_existing_checker_valid_objects_case() {
  local case_dir="${tmp_root}/existing-checker-valid"

  mkdir -p "${case_dir}"
  seed_existing_release_objects "${case_dir}"
  run_existing_checker "${case_dir}" >"${case_dir}/output.log" 2>&1
  grep -q 'runtime_asset_exists=true' "${case_dir}/output.log"
  if grep -q '^put ' "${case_dir}/aws.log"; then
    echo "existing checker uploaded despite existing release objects" >&2
    exit 1
  fi
  if [[ -s "${case_dir}/curl.log" ]]; then
    echo "existing checker touched public CDN despite S3-only preflight" >&2
    exit 1
  fi
}

assert_existing_checker_invalid_metadata_fails_case() {
  local case_dir="${tmp_root}/existing-checker-invalid-metadata"
  local asset
  local asset_key
  local asset_object

  mkdir -p "${case_dir}"
  seed_existing_release_objects "${case_dir}"
  asset="$(asset_path_for "${case_dir}")"
  asset_key="$(runtime_key_for_asset "${asset}")"
  asset_object="$(object_path_for_key "${case_dir}" "${asset_key}")"
  : >"$(metadata_path_for "${asset_object}")"

  if run_existing_checker "${case_dir}" >"${case_dir}/output.log" 2>&1; then
    echo "expected existing checker invalid metadata case to fail" >&2
    exit 1
  fi
  grep -q 'invalid immutable metadata' "${case_dir}/output.log"
  if grep -q '^put ' "${case_dir}/aws.log"; then
    echo "existing checker uploaded despite invalid existing metadata" >&2
    exit 1
  fi
  if [[ -s "${case_dir}/curl.log" ]]; then
    echo "existing checker touched public CDN despite invalid metadata" >&2
    exit 1
  fi
}

install_stubs "${tmp_root}/bin"
assert_github_actions_guard_case
assert_success_case
assert_public_head_retry_case
assert_public_missing_metadata_failure_case
assert_public_mismatched_metadata_failure_case
assert_existing_matching_metadata_skips_upload_case
assert_existing_missing_metadata_fails_case
assert_existing_mismatched_metadata_fails_case
assert_head_miss_concurrent_put_race_case
assert_existing_checker_missing_objects_case
assert_existing_checker_valid_objects_case
assert_existing_checker_invalid_metadata_fails_case

echo "Runtime DSUB S3 publisher validation passed."
