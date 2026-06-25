#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
publisher="${repo_root}/scripts/publish-runtime-asset-s3.sh"
tmp_root="$(mktemp -d)"
target="x86_64-unknown-linux-gnu"
version="1.2.3"
release_tag="v1.2.3"
bucket="skenion"
prefix="releases"
public_base="https://cdn.example.test/skenion/releases"

cleanup() {
  rm -rf "${tmp_root}"
}
trap cleanup EXIT

write_checksum() {
  local asset="$1"
  local output="$2"

  if command -v sha256sum >/dev/null 2>&1; then
    (
      cd "$(dirname "${asset}")"
      sha256sum "$(basename "${asset}")" >"${output}"
    )
  elif command -v shasum >/dev/null 2>&1; then
    (
      cd "$(dirname "${asset}")"
      shasum -a 256 "$(basename "${asset}")" >"${output}"
    )
  else
    echo "no sha256 checksum tool found" >&2
    exit 1
  fi
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

metadata_sha_of() {
  local path="$1"
  if [[ -f "${path}.stub-sha256" ]]; then
    sed -n '1p' "${path}.stub-sha256"
  fi
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

        sha="$(metadata_sha_of "${path}")"
        if [[ "${STUB_AWS_CORRUPT_SHA_ON_HEAD:-}" == "1" ]]; then
          sha="0000000000000000000000000000000000000000000000000000000000000000"
        fi
        if [[ "${STUB_AWS_DROP_METADATA_ON_HEAD:-}" == "1" ]]; then
          printf '{"ContentLength":%s,"Metadata":{}}\n' "$(size_of "${path}")"
        else
          printf '{"ContentLength":%s,"Metadata":{"sha256":"%s"}}\n' "$(size_of "${path}")" "${sha}"
        fi
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
          printf 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n' >"${path}.stub-sha256"
        fi
        if [[ -f "${path}" ]]; then
          echo "An error occurred (PreconditionFailed) when calling the PutObject operation: At least one of the pre-conditions you specified did not hold" >&2
          echo "put-precondition-failed ${bucket}/${key}" >>"${log}"
          exit 255
        fi

        command cp "${body}" "${path}"
        sha=""
        IFS=',' read -r -a pairs <<<"${metadata}"
        for pair in "${pairs[@]}"; do
          if [[ "${pair}" == sha256=* ]]; then
            sha="${pair#sha256=}"
          fi
        done
        printf '%s\n' "${sha}" >"${path}.stub-sha256"
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
    if [[ "${subcommand}" != "cp" ]]; then
      echo "unsupported aws s3 subcommand: ${subcommand}" >&2
      exit 2
    fi

    src="$1"
    dest="$2"
    shift 2
    metadata=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --metadata)
          metadata="$2"
          shift 2
          ;;
        --content-type)
          shift 2
          ;;
        --no-progress)
          shift
          ;;
        *)
          shift
          ;;
      esac
    done

    if [[ "${src}" == s3://* ]]; then
      bucket_and_key="${src#s3://}"
      bucket="${bucket_and_key%%/*}"
      key="${bucket_and_key#*/}"
      path="${root}/${bucket}/${key}"
      if [[ ! -f "${path}" ]]; then
        echo "download source missing: ${src}" >&2
        exit 255
      fi
      if [[ "${STUB_AWS_CORRUPT_S3_DOWNLOAD:-}" == "1" ]]; then
        printf 'corrupt authenticated download\n' >"${dest}"
      else
        command cp "${path}" "${dest}"
      fi
      echo "get ${bucket}/${key}" >>"${log}"
    else
      bucket_and_key="${dest#s3://}"
      bucket="${bucket_and_key%%/*}"
      key="${bucket_and_key#*/}"
      path="${root}/${bucket}/${key}"
      mkdir -p "$(dirname "${path}")"
      command cp "${src}" "${path}"

      sha=""
      IFS=',' read -r -a pairs <<<"${metadata}"
      for pair in "${pairs[@]}"; do
        if [[ "${pair}" == sha256=* ]]; then
          sha="${pair#sha256=}"
        fi
      done
      printf '%s\n' "${sha}" >"${path}.stub-sha256"
      echo "cp ${bucket}/${key}" >>"${log}"
    fi
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
method="GET"
output=""
dump_header=""
url=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --head|-I)
      method="HEAD"
      shift
      ;;
    --output|-o)
      output="$2"
      shift 2
      ;;
    --dump-header|-D)
      dump_header="$2"
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

if [[ "${method}" == "HEAD" ]]; then
  if [[ -n "${dump_header}" ]]; then
    printf 'HTTP/1.1 200 OK\r\nContent-Length: %s\r\n\r\n' "${size}" >"${dump_header}"
  else
    printf 'HTTP/1.1 200 OK\r\nContent-Length: %s\r\n\r\n' "${size}"
  fi
  exit 0
fi

if [[ "${STUB_CURL_CORRUPT_CHECKSUM:-}" == "1" && "${relative_key}" == *.sha256 ]]; then
  body="corrupt checksum response"
  if [[ -n "${output}" ]]; then
    printf '%s\n' "${body}" >"${output}"
  else
    printf '%s\n' "${body}"
  fi
  exit 0
fi

if [[ "${STUB_CURL_CORRUPT_MANIFEST:-}" == "1" && "${relative_key}" == *.manifest.json ]]; then
  body='{"corrupt":true}'
  if [[ -n "${output}" ]]; then
    printf '%s\n' "${body}" >"${output}"
  else
    printf '%s\n' "${body}"
  fi
  exit 0
fi

if [[ -n "${output}" ]]; then
  command cp "${path}" "${output}"
else
  cat "${path}"
fi
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
    "SOURCE_COMMIT=1111111111111111111111111111111111111111"
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
    assert any(event == f"get {key}" for event in events[index + 1 :]), (key, events)
PY

  grep -q '^HEAD skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.tar\.gz$' "${case_dir}/curl.log"
  grep -q '^GET skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.sha256$' "${case_dir}/curl.log"
  grep -q '^GET skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.manifest\.json$' "${case_dir}/curl.log"
}

assert_no_clobber_case() {
  local case_dir="${tmp_root}/no-clobber"
  local asset
  local asset_name
  local existing

  prepare_case "${case_dir}" "runtime no-clobber artifact"
  asset="$(asset_path_for "${case_dir}")"
  asset_name="$(basename "${asset}")"
  existing="${case_dir}/s3/${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}"
  mkdir -p "$(dirname "${existing}")"
  printf 'different existing asset\n' >"${existing}"
  printf 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n' >"${existing}.stub-sha256"

  if run_publisher "${case_dir}" >"${case_dir}/output.log" 2>&1; then
    echo "expected no-clobber publisher case to fail" >&2
    exit 1
  fi

  grep -q 'refusing to overwrite existing Runtime release artifact' "${case_dir}/output.log"
  if grep -q '^put ' "${case_dir}/aws.log"; then
    echo "publisher uploaded despite no-clobber refusal" >&2
    exit 1
  fi
}

assert_missing_metadata_download_verification_case() {
  local case_dir="${tmp_root}/missing-metadata"

  prepare_case "${case_dir}" "runtime missing metadata artifact"
  run_publisher "${case_dir}" STUB_AWS_DROP_METADATA_ON_HEAD=1 >"${case_dir}/output.log" 2>&1

  grep -q 'content matches by authenticated S3 download; head metadata sha256=<missing>' "${case_dir}/output.log"
  grep -q '^get skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.tar\.gz$' "${case_dir}/aws.log"
  grep -q '^get skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.sha256$' "${case_dir}/aws.log"
  grep -q '^get skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/.*\.manifest\.json$' "${case_dir}/aws.log"
}

assert_existing_object_missing_metadata_same_content_case() {
  local case_dir="${tmp_root}/existing-missing-metadata"
  local asset
  local asset_name
  local existing

  prepare_case "${case_dir}" "runtime existing missing metadata artifact"
  asset="$(asset_path_for "${case_dir}")"
  asset_name="$(basename "${asset}")"
  existing="${case_dir}/s3/${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}"
  mkdir -p "$(dirname "${existing}")"
  command cp "${asset}" "${existing}"

  run_publisher "${case_dir}" STUB_AWS_DROP_METADATA_ON_HEAD=1 >"${case_dir}/output.log" 2>&1

  grep -q 'object already exists with matching content' "${case_dir}/output.log"
  grep -q "^get skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/${asset_name}$" "${case_dir}/aws.log"
  if grep -q "^cp skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/${asset_name}$" "${case_dir}/aws.log"; then
    echo "publisher overwrote existing asset despite authenticated content match" >&2
    exit 1
  fi
  if grep -q "^put skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/${asset_name}$" "${case_dir}/aws.log"; then
    echo "publisher overwrote existing asset despite authenticated content match" >&2
    exit 1
  fi
}

assert_existing_object_matching_metadata_different_body_case() {
  local case_dir="${tmp_root}/matching-metadata-different-body"
  local asset
  local asset_name
  local asset_sha
  local existing

  prepare_case "${case_dir}" "aaaaaaaaaa"
  asset="$(asset_path_for "${case_dir}")"
  asset_name="$(basename "${asset}")"
  asset_sha="$(awk '{print $1; exit}' "${asset}.sha256")"
  existing="${case_dir}/s3/${bucket}/${prefix}/skenion-runtime/${release_tag}/${target}/${asset_name}"
  mkdir -p "$(dirname "${existing}")"
  printf 'bbbbbbbbbb\n' >"${existing}"
  printf '%s\n' "${asset_sha}" >"${existing}.stub-sha256"

  if run_publisher "${case_dir}" >"${case_dir}/output.log" 2>&1; then
    echo "expected matching metadata with different body case to fail" >&2
    exit 1
  fi

  grep -q 'content does not match local file after authenticated S3 download' "${case_dir}/output.log"
  grep -q 'refusing to overwrite existing Runtime release artifact' "${case_dir}/output.log"
  if grep -q "^put skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/${asset_name}$" "${case_dir}/aws.log"; then
    echo "publisher overwrote existing asset despite body mismatch" >&2
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

  grep -q 'failed to conditionally upload Runtime release artifact without overwriting' "${case_dir}/output.log"
  grep -q 'PreconditionFailed' "${case_dir}/output.log"
  grep -q "^put-precondition-failed skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/${asset_name}$" "${case_dir}/aws.log"
  if grep -q "^put skenion/releases/skenion-runtime/v1.2.3/x86_64-unknown-linux-gnu/${asset_name}$" "${case_dir}/aws.log"; then
    echo "publisher overwrote concurrent object despite conditional put failure" >&2
    exit 1
  fi
  grep -q '^concurrent object$' "${raced_path}"
}

assert_authenticated_download_failure_case() {
  local case_dir="${tmp_root}/authenticated-download-failure"

  prepare_case "${case_dir}" "runtime authenticated download failure artifact"
  if run_publisher "${case_dir}" STUB_AWS_DROP_METADATA_ON_HEAD=1 STUB_AWS_CORRUPT_S3_DOWNLOAD=1 >"${case_dir}/output.log" 2>&1; then
    echo "expected authenticated download verification case to fail" >&2
    exit 1
  fi

  grep -q 'content does not match local file after authenticated S3 download' "${case_dir}/output.log"
}

assert_public_checksum_failure_case() {
  local case_dir="${tmp_root}/checksum-failure"

  prepare_case "${case_dir}" "runtime public checksum failure artifact"
  if run_publisher "${case_dir}" STUB_CURL_CORRUPT_CHECKSUM=1 >"${case_dir}/output.log" 2>&1; then
    echo "expected public checksum verification case to fail" >&2
    exit 1
  fi

  grep -q 'public Runtime release .*checksum.* content does not match local file' "${case_dir}/output.log"
}

assert_public_manifest_failure_case() {
  local case_dir="${tmp_root}/manifest-failure"

  prepare_case "${case_dir}" "runtime public manifest failure artifact"
  if run_publisher "${case_dir}" STUB_CURL_CORRUPT_MANIFEST=1 >"${case_dir}/output.log" 2>&1; then
    echo "expected public manifest verification case to fail" >&2
    exit 1
  fi

  grep -q 'public Runtime release .*manifest.* content does not match local file' "${case_dir}/output.log"
}

install_stubs "${tmp_root}/bin"
assert_github_actions_guard_case
assert_success_case
assert_no_clobber_case
assert_missing_metadata_download_verification_case
assert_existing_object_missing_metadata_same_content_case
assert_existing_object_matching_metadata_different_body_case
assert_head_miss_concurrent_put_race_case
assert_authenticated_download_failure_case
assert_public_checksum_failure_case
assert_public_manifest_failure_case

echo "Runtime DSUB S3 publisher validation passed."
