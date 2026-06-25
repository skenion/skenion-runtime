#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target-triple> <version> <release-tag>" >&2
  exit 2
fi

target="$1"
version="$2"
release_tag="$3"

runtime_platform_slug() {
  case "$1" in
    aarch64-apple-darwin)
      printf '%s' "macos-apple-silicon"
      ;;
    x86_64-apple-darwin)
      printf '%s' "macos-intel"
      ;;
    x86_64-pc-windows-msvc)
      printf '%s' "windows-x64"
      ;;
    aarch64-pc-windows-msvc)
      printf '%s' "windows-arm64"
      ;;
    x86_64-unknown-linux-gnu)
      printf '%s' "linux-x64"
      ;;
    aarch64-unknown-linux-gnu)
      printf '%s' "linux-arm64"
      ;;
    *)
      echo "unsupported Runtime release target triple: $1" >&2
      exit 1
      ;;
  esac
}

runtime_archive_extension() {
  case "$1" in
    *windows*)
      printf '%s' "zip"
      ;;
    *)
      printf '%s' "tar.gz"
      ;;
  esac
}

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "required environment variable is missing: ${name}" >&2
    exit 1
  fi
}

trim_slashes() {
  local value="$1"
  value="${value#/}"
  value="${value%/}"
  printf '%s' "${value}"
}

join_key() {
  local prefix="$1"
  local suffix="$2"
  if [[ -n "${prefix}" ]]; then
    printf '%s/%s' "${prefix}" "${suffix}"
  else
    printf '%s' "${suffix}"
  fi
}

relative_public_key() {
  local prefix="$1"
  local key="$2"
  if [[ -n "${prefix}" && "${key}" == "${prefix}/"* ]]; then
    printf '%s' "${key#"${prefix}/"}"
  else
    printf '%s' "${key}"
  fi
}

public_url() {
  local relative_key="$1"
  printf '%s/%s' "${SKENION_RELEASE_PUBLIC_BASE_URL%/}" "${relative_key#/}"
}

for env_name in \
  SKENION_RELEASE_S3_ENDPOINT \
  SKENION_RELEASE_S3_REGION \
  SKENION_RELEASE_S3_BUCKET \
  SKENION_RELEASE_S3_PREFIX \
  SKENION_RELEASE_S3_ACCESS_KEY_ID \
  SKENION_RELEASE_S3_SECRET_ACCESS_KEY \
  SKENION_RELEASE_S3_FORCE_PATH_STYLE \
  SKENION_RELEASE_PUBLIC_BASE_URL; do
  require_env "${env_name}"
done

if ! command -v aws >/dev/null 2>&1; then
  echo "aws CLI is required for Runtime release artifact existence checks." >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required for Runtime release artifact existence checks." >&2
  exit 1
fi

export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-${SKENION_RELEASE_S3_ACCESS_KEY_ID}}"
export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-${SKENION_RELEASE_S3_SECRET_ACCESS_KEY}}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-${SKENION_RELEASE_S3_REGION}}"
export AWS_PAGER=""

if [[ "${SKENION_RELEASE_S3_FORCE_PATH_STYLE:-}" =~ ^(1|true|TRUE|yes|YES)$ ]]; then
  aws configure set default.s3.addressing_style path
fi

platform_slug="$(runtime_platform_slug "${target}")"
archive_extension="$(runtime_archive_extension "${target}")"
asset_name="skenion-runtime-v${version}-${platform_slug}.${archive_extension}"
checksum_name="${asset_name}.sha256"
manifest_name="${asset_name}.manifest.json"
prefix="$(trim_slashes "${SKENION_RELEASE_S3_PREFIX}")"
artifact_dir="$(join_key "${prefix}" "skenion-runtime/${release_tag}/${platform_slug}")"
asset_key="$(join_key "${artifact_dir}" "${asset_name}")"
checksum_key="$(join_key "${artifact_dir}" "${checksum_name}")"
manifest_key="$(join_key "${artifact_dir}" "${manifest_name}")"
asset_url="$(public_url "$(relative_public_key "${prefix}" "${asset_key}")")"
checksum_url="$(public_url "$(relative_public_key "${prefix}" "${checksum_key}")")"
manifest_url="$(public_url "$(relative_public_key "${prefix}" "${manifest_key}")")"

head_json="$(mktemp)"
head_err="$(mktemp)"
cleanup() {
  rm -f "${head_json}" "${head_err}"
}
trap cleanup EXIT

read_head_field() {
  local field="$1"
  local path="$2"

  python3 - "${field}" "${path}" <<'PY'
import json
import sys

field = sys.argv[1]
with open(sys.argv[2], encoding="utf-8") as fh:
    head = json.load(fh)

metadata = head.get("Metadata") or {}

if field in {"sha256", "component", "target", "runtime-version", "source-tag", "source-commit"}:
    print(metadata.get(field, ""))
elif field == "size":
    print(head.get("ContentLength", ""))
else:
    raise SystemExit(f"unsupported head field: {field}")
PY
}

summarize_existing_metadata() {
  local key="$1"
  local label="$2"
  local actual_sha
  local actual_size
  local actual_component
  local actual_target
  local actual_version
  local actual_tag
  local actual_commit

  actual_sha="$(read_head_field sha256 "${head_json}")"
  actual_size="$(read_head_field size "${head_json}")"
  actual_component="$(read_head_field component "${head_json}")"
  actual_target="$(read_head_field target "${head_json}")"
  actual_version="$(read_head_field runtime-version "${head_json}")"
  actual_tag="$(read_head_field source-tag "${head_json}")"
  actual_commit="$(read_head_field source-commit "${head_json}")"

  if [[ -z "${actual_size}" ]]; then
    echo "existing Runtime release ${label} is missing ContentLength: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
    exit 1
  fi

  if [[ -n "${actual_sha}${actual_component}${actual_target}${actual_version}${actual_tag}${actual_commit}" ]]; then
    echo "found existing Runtime release ${label} with S3 metadata: s3://${SKENION_RELEASE_S3_BUCKET}/${key}"
  else
    echo "found existing Runtime release ${label} without S3 metadata: s3://${SKENION_RELEASE_S3_BUCKET}/${key}"
  fi
}

object_exists_or_missing() {
  local key="$1"
  local label="$2"

  if aws --endpoint-url "${SKENION_RELEASE_S3_ENDPOINT}" s3api head-object \
    --bucket "${SKENION_RELEASE_S3_BUCKET}" \
    --key "${key}" >"${head_json}" 2>"${head_err}"; then
    summarize_existing_metadata "${key}" "${label}"
    return 0
  fi

  if grep -Eiq '(404|Not Found|NoSuchKey|NotFound)' "${head_err}"; then
    echo "missing Runtime release ${label}: s3://${SKENION_RELEASE_S3_BUCKET}/${key}"
    return 1
  fi

  echo "failed to inspect Runtime release ${label}: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
  cat "${head_err}" >&2
  exit 1
}

missing=false
object_exists_or_missing "${asset_key}" "asset" || missing=true
object_exists_or_missing "${checksum_key}" "checksum" || missing=true
object_exists_or_missing "${manifest_key}" "manifest" || missing=true

exists=false
if [[ "${missing}" != "true" ]]; then
  exists=true
fi

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    echo "exists=${exists}"
    echo "asset_url=${asset_url}"
    echo "checksum_url=${checksum_url}"
    echo "manifest_url=${manifest_url}"
    echo "asset_key=${asset_key}"
    echo "checksum_key=${checksum_key}"
    echo "manifest_key=${manifest_key}"
    echo "asset_name=${asset_name}"
    echo "checksum_name=${checksum_name}"
    echo "manifest_name=${manifest_name}"
    echo "platform_slug=${platform_slug}"
    echo "archive_extension=${archive_extension}"
  } >>"${GITHUB_OUTPUT}"
fi

echo "runtime_asset_exists=${exists}"
