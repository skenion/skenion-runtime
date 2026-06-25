#!/usr/bin/env bash
set -euo pipefail

dry_run=false
use_existing_manifest=false
skip_public_verification=false
while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --dry-run)
      dry_run=true
      ;;
    --use-existing-manifest)
      use_existing_manifest=true
      ;;
    --skip-public-verification)
      skip_public_verification=true
      ;;
    *)
      echo "unknown option: $1" >&2
      exit 2
      ;;
  esac
  shift
done

if [[ $# -ne 5 ]]; then
  echo "usage: $0 [--dry-run] [--use-existing-manifest] [--skip-public-verification] <target-triple> <version> <release-tag> <asset-path> <checksum-path>" >&2
  exit 2
fi

target="$1"
version="$2"
release_tag="$3"
asset_path="$4"
checksum_path="$5"

find_python() {
  if command -v python3 >/dev/null 2>&1; then
    command -v python3
  elif command -v python >/dev/null 2>&1; then
    command -v python
  else
    echo "python3 or python is required for Runtime release manifest generation." >&2
    exit 1
  fi
}

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "required environment variable is missing: ${name}" >&2
    exit 1
  fi
}

require_github_actions_publish_context() {
  if [[ "${GITHUB_ACTIONS:-}" != "true" ]]; then
    echo "Runtime release artifact publishing must run from GitHub Actions." >&2
    exit 1
  fi

  case "${GITHUB_EVENT_NAME:-}" in
    release | workflow_dispatch)
      ;;
    *)
      echo "Runtime release artifact publishing is only allowed for release or workflow_dispatch events." >&2
      exit 1
      ;;
  esac
}

require_file() {
  local path="$1"
  if [[ ! -f "${path}" ]]; then
    echo "required file does not exist: ${path}" >&2
    exit 1
  fi
}

file_size() {
  local path="$1"
  if stat -c '%s' "${path}" >/dev/null 2>&1; then
    stat -c '%s' "${path}"
  else
    stat -f '%z' "${path}"
  fi
}

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

if [[ "${dry_run}" != "true" ]]; then
  require_github_actions_publish_context
fi

require_file "${asset_path}"
require_file "${checksum_path}"
python_bin="$(find_python)"

for env_name in \
  SKENION_RELEASE_S3_ENDPOINT \
  SKENION_RELEASE_S3_REGION \
  SKENION_RELEASE_S3_BUCKET \
  SKENION_RELEASE_S3_PREFIX \
  SKENION_RELEASE_S3_FORCE_PATH_STYLE \
  SKENION_RELEASE_PUBLIC_BASE_URL; do
  require_env "${env_name}"
done

if [[ "${dry_run}" != "true" ]]; then
  require_env SKENION_RELEASE_S3_ACCESS_KEY_ID
  require_env SKENION_RELEASE_S3_SECRET_ACCESS_KEY
fi

asset_name="$(basename "${asset_path}")"
checksum_name="$(basename "${checksum_path}")"
manifest_name="${asset_name}.manifest.json"
manifest_path="$(dirname "${asset_path}")/${manifest_name}"
source_commit="${SOURCE_COMMIT:-${GITHUB_SHA:-unknown}}"
release_tier="${RELEASE_TIER:-unknown}"
contracts_version="${CONTRACTS_VERSION:-}"
contracts_line="${CONTRACTS_LINE:-}"

asset_sha="$(sha256_file "${asset_path}")"
declared_asset_sha="$(awk '{print $1; exit}' "${checksum_path}")"
if [[ "${asset_sha}" != "${declared_asset_sha}" ]]; then
  echo "checksum file does not match asset: ${checksum_path}" >&2
  echo "asset sha256: ${asset_sha}" >&2
  echo "declared sha256: ${declared_asset_sha}" >&2
  exit 1
fi

asset_size="$(file_size "${asset_path}")"
prefix="$(trim_slashes "${SKENION_RELEASE_S3_PREFIX}")"
artifact_dir="$(join_key "${prefix}" "skenion-runtime/${release_tag}/${target}")"
asset_key="$(join_key "${artifact_dir}" "${asset_name}")"
checksum_key="$(join_key "${artifact_dir}" "${checksum_name}")"
manifest_key="$(join_key "${artifact_dir}" "${manifest_name}")"
asset_url="$(public_url "$(relative_public_key "${prefix}" "${asset_key}")")"
checksum_url="$(public_url "$(relative_public_key "${prefix}" "${checksum_key}")")"
manifest_url="$(public_url "$(relative_public_key "${prefix}" "${manifest_key}")")"

export RUNTIME_RELEASE_COMPONENT="skenion-runtime"
export RUNTIME_RELEASE_SCHEMA="skenion.runtime.releaseArtifact.v1"
export RUNTIME_RELEASE_VERSION="${version}"
export RUNTIME_RELEASE_TAG="${release_tag}"
export RUNTIME_RELEASE_TARGET="${target}"
export RUNTIME_RELEASE_TIER="${release_tier}"
export RUNTIME_RELEASE_SOURCE_COMMIT="${source_commit}"
export RUNTIME_RELEASE_CONTRACTS_VERSION="${contracts_version}"
export RUNTIME_RELEASE_CONTRACTS_LINE="${contracts_line}"
export RUNTIME_RELEASE_BUCKET="${SKENION_RELEASE_S3_BUCKET}"
export RUNTIME_RELEASE_ASSET_NAME="${asset_name}"
export RUNTIME_RELEASE_ASSET_KEY="${asset_key}"
export RUNTIME_RELEASE_ASSET_URL="${asset_url}"
export RUNTIME_RELEASE_ASSET_SHA256="${asset_sha}"
export RUNTIME_RELEASE_ASSET_SIZE="${asset_size}"
export RUNTIME_RELEASE_CHECKSUM_NAME="${checksum_name}"
export RUNTIME_RELEASE_CHECKSUM_KEY="${checksum_key}"
export RUNTIME_RELEASE_CHECKSUM_URL="${checksum_url}"
export RUNTIME_RELEASE_MANIFEST_NAME="${manifest_name}"
export RUNTIME_RELEASE_MANIFEST_KEY="${manifest_key}"
export RUNTIME_RELEASE_MANIFEST_URL="${manifest_url}"

write_manifest() {
  local output_path="$1"

  "${python_bin}" - "${output_path}" <<'PY'
import json
import os
import sys

manifest = {
    "schema": os.environ["RUNTIME_RELEASE_SCHEMA"],
    "component": os.environ["RUNTIME_RELEASE_COMPONENT"],
    "runtimeVersion": os.environ["RUNTIME_RELEASE_VERSION"],
    "releaseTag": os.environ["RUNTIME_RELEASE_TAG"],
    "sourceCommit": os.environ["RUNTIME_RELEASE_SOURCE_COMMIT"],
    "target": os.environ["RUNTIME_RELEASE_TARGET"],
    "tier": os.environ["RUNTIME_RELEASE_TIER"],
    "contracts": {
        "version": os.environ.get("RUNTIME_RELEASE_CONTRACTS_VERSION") or None,
        "line": os.environ.get("RUNTIME_RELEASE_CONTRACTS_LINE") or None,
    },
    "artifact": {
        "filename": os.environ["RUNTIME_RELEASE_ASSET_NAME"],
        "sha256": os.environ["RUNTIME_RELEASE_ASSET_SHA256"],
        "size": int(os.environ["RUNTIME_RELEASE_ASSET_SIZE"]),
        "s3": {
            "bucket": os.environ["RUNTIME_RELEASE_BUCKET"],
            "key": os.environ["RUNTIME_RELEASE_ASSET_KEY"],
        },
        "publicUrl": os.environ["RUNTIME_RELEASE_ASSET_URL"],
    },
    "checksum": {
        "filename": os.environ["RUNTIME_RELEASE_CHECKSUM_NAME"],
        "s3": {
            "bucket": os.environ["RUNTIME_RELEASE_BUCKET"],
            "key": os.environ["RUNTIME_RELEASE_CHECKSUM_KEY"],
        },
        "publicUrl": os.environ["RUNTIME_RELEASE_CHECKSUM_URL"],
    },
    "manifest": {
        "filename": os.environ["RUNTIME_RELEASE_MANIFEST_NAME"],
        "s3": {
            "bucket": os.environ["RUNTIME_RELEASE_BUCKET"],
            "key": os.environ["RUNTIME_RELEASE_MANIFEST_KEY"],
        },
        "publicUrl": os.environ["RUNTIME_RELEASE_MANIFEST_URL"],
    },
}

with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump(manifest, fh, indent=2, sort_keys=True)
    fh.write("\n")
PY
}

if [[ "${use_existing_manifest}" == "true" ]]; then
  require_file "${manifest_path}"
  expected_manifest_path="$(mktemp)"
  write_manifest "${expected_manifest_path}"
  if ! cmp -s "${manifest_path}" "${expected_manifest_path}"; then
    rm -f "${expected_manifest_path}"
    echo "existing Runtime release manifest does not match current release inputs: ${manifest_path}" >&2
    exit 1
  fi
  rm -f "${expected_manifest_path}"
else
  write_manifest "${manifest_path}"
fi

checksum_sha="$(sha256_file "${checksum_path}")"
checksum_size="$(file_size "${checksum_path}")"
manifest_sha="$(sha256_file "${manifest_path}")"
manifest_size="$(file_size "${manifest_path}")"

if [[ "${dry_run}" == "true" ]]; then
  echo "dry run: would publish runtime asset to ${asset_url}"
  echo "dry run: would publish checksum to ${checksum_url}"
  echo "dry run: would publish manifest to ${manifest_url}"
else
  if ! command -v aws >/dev/null 2>&1; then
    echo "aws CLI is required for Runtime release artifact publishing." >&2
    exit 1
  fi
  if [[ "${skip_public_verification}" != "true" ]] && ! command -v curl >/dev/null 2>&1; then
    echo "curl is required for Runtime release artifact public-read verification." >&2
    exit 1
  fi

  export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-${SKENION_RELEASE_S3_ACCESS_KEY_ID}}"
  export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-${SKENION_RELEASE_S3_SECRET_ACCESS_KEY}}"
  export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-${SKENION_RELEASE_S3_REGION}}"
  export AWS_PAGER=""

  if [[ "${SKENION_RELEASE_S3_FORCE_PATH_STYLE:-}" =~ ^(1|true|TRUE|yes|YES)$ ]]; then
    aws configure set default.s3.addressing_style path
  fi

  public_verify_attempts="${SKENION_PUBLIC_VERIFY_ATTEMPTS:-24}"
  public_verify_sleep_seconds="${SKENION_PUBLIC_VERIFY_SLEEP_SECONDS:-5}"
  if [[ ! "${public_verify_attempts}" =~ ^[1-9][0-9]*$ ]]; then
    echo "SKENION_PUBLIC_VERIFY_ATTEMPTS must be a positive integer." >&2
    exit 1
  fi
  if [[ ! "${public_verify_sleep_seconds}" =~ ^[0-9]+$ ]]; then
    echo "SKENION_PUBLIC_VERIFY_SLEEP_SECONDS must be a non-negative integer." >&2
    exit 1
  fi

  head_json="$(mktemp)"
  head_err="$(mktemp)"
  cleanup_head() {
    rm -f "${head_json}" "${head_err}"
  }
  trap cleanup_head EXIT

  read_s3_head_field() {
    local field="$1"
    local path="$2"

    "${python_bin}" - "${field}" "${path}" <<'PY'
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

  s3_head_metadata_matches_expected() {
    local key="$1"
    local expected_sha="$2"
    local expected_size="$3"
    local label="$4"
    local actual_sha
    local actual_size
    local actual_component
    local actual_target
    local actual_version
    local actual_tag
    local actual_commit

    actual_sha="$(read_s3_head_field sha256 "${head_json}")"
    actual_size="$(read_s3_head_field size "${head_json}")"
    actual_component="$(read_s3_head_field component "${head_json}")"
    actual_target="$(read_s3_head_field target "${head_json}")"
    actual_version="$(read_s3_head_field runtime-version "${head_json}")"
    actual_tag="$(read_s3_head_field source-tag "${head_json}")"
    actual_commit="$(read_s3_head_field source-commit "${head_json}")"

    if [[ "${actual_sha}" == "${expected_sha}" \
      && "${actual_size}" == "${expected_size}" \
      && "${actual_component}" == "skenion-runtime" \
      && "${actual_target}" == "${target}" \
      && "${actual_version}" == "${version}" \
      && "${actual_tag}" == "${release_tag}" \
      && "${actual_commit}" == "${source_commit}" ]]; then
      return 0
    fi

    echo "Runtime release ${label} S3 metadata does not match expected immutable artifact: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
    echo "expected sha256=${expected_sha} size=${expected_size} component=skenion-runtime target=${target} runtime-version=${version} source-tag=${release_tag} source-commit=${source_commit}" >&2
    echo "actual sha256=${actual_sha:-<missing>} size=${actual_size:-<missing>} component=${actual_component:-<missing>} target=${actual_target:-<missing>} runtime-version=${actual_version:-<missing>} source-tag=${actual_tag:-<missing>} source-commit=${actual_commit:-<missing>}" >&2
    return 1
  }

  object_exists_with_same_metadata() {
    local key="$1"
    local expected_sha="$2"
    local expected_size="$3"

    if aws --endpoint-url "${SKENION_RELEASE_S3_ENDPOINT}" s3api head-object \
      --bucket "${SKENION_RELEASE_S3_BUCKET}" \
      --key "${key}" >"${head_json}" 2>"${head_err}"; then
      if s3_head_metadata_matches_expected "${key}" "${expected_sha}" "${expected_size}" "existing object"; then
        echo "object already exists with matching immutable metadata: s3://${SKENION_RELEASE_S3_BUCKET}/${key}"
        return 0
      fi
      echo "refusing to overwrite existing Runtime release artifact: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
      exit 1
    fi

    if grep -Eiq '(404|Not Found|NoSuchKey|NotFound)' "${head_err}"; then
      return 1
    fi

    echo "failed to inspect release artifact object: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
    cat "${head_err}" >&2
    exit 1
  }

  verify_s3_object_metadata() {
    local key="$1"
    local expected_sha="$2"
    local expected_size="$3"
    local label="$4"

    if ! aws --endpoint-url "${SKENION_RELEASE_S3_ENDPOINT}" s3api head-object \
      --bucket "${SKENION_RELEASE_S3_BUCKET}" \
      --key "${key}" >"${head_json}" 2>"${head_err}"; then
      echo "failed to verify uploaded Runtime release ${label}: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
      cat "${head_err}" >&2
      exit 1
    fi

    if ! s3_head_metadata_matches_expected "${key}" "${expected_sha}" "${expected_size}" "${label}"; then
      exit 1
    fi
  }

  upload_object() {
    local path="$1"
    local key="$2"
    local sha="$3"
    local size="$4"
    local content_type="$5"

    if object_exists_with_same_metadata "${key}" "${sha}" "${size}"; then
      return 0
    fi

    if ! aws --endpoint-url "${SKENION_RELEASE_S3_ENDPOINT}" s3api put-object \
      --bucket "${SKENION_RELEASE_S3_BUCKET}" \
      --key "${key}" \
      --body "${path}" \
      --content-type "${content_type}" \
      --metadata "sha256=${sha},component=skenion-runtime,target=${target},runtime-version=${version},source-tag=${release_tag},source-commit=${source_commit}" \
      --if-none-match '*' >/dev/null 2>"${head_err}"; then
      echo "failed to conditionally upload Runtime release artifact without overwriting: s3://${SKENION_RELEASE_S3_BUCKET}/${key}" >&2
      cat "${head_err}" >&2
      exit 1
    fi

    verify_s3_object_metadata "${key}" "${sha}" "${size}" "$(basename "${path}")"
  }

  read_public_header() {
    local name="$1"
    local path="$2"

    awk -v header_name="${name}" '
      BEGIN { IGNORECASE = 1 }
      {
        line = $0
        gsub("\r", "", line)
        split(line, parts, ":")
        if (tolower(parts[1]) == tolower(header_name)) {
          sub("^[^:]*:[[:space:]]*", "", line)
          value = line
        }
      }
      END { print value }
    ' "${path}"
  }

  public_head_matches_expected() {
    local url="$1"
    local expected_sha="$2"
    local expected_size="$3"
    local label="$4"
    local actual_size
    local actual_sha

    actual_size="$(read_public_header "Content-Length" "${head_json}")"
    actual_sha="$(read_public_header "x-amz-meta-sha256" "${head_json}")"

    if [[ -z "${actual_size}" ]]; then
      echo "public Runtime release ${label} is missing Content-Length: ${url}" >&2
      return 1
    fi
    if [[ -z "${actual_sha}" ]]; then
      echo "public Runtime release ${label} is missing x-amz-meta-sha256; CDN must expose S3 user metadata: ${url}" >&2
      return 1
    fi
    if [[ "${actual_size}" != "${expected_size}" || "${actual_sha}" != "${expected_sha}" ]]; then
      echo "public Runtime release ${label} HEAD metadata does not match local immutable artifact: ${url}" >&2
      echo "expected sha256=${expected_sha} size=${expected_size}" >&2
      echo "actual sha256=${actual_sha:-<missing>} size=${actual_size:-<missing>}" >&2
      return 1
    fi

    return 0
  }

  verify_public_head_metadata() {
    local url="$1"
    local expected_sha="$2"
    local expected_size="$3"
    local label="$4"
    local attempt
    local last_error

    for ((attempt = 1; attempt <= public_verify_attempts; attempt++)); do
      : >"${head_json}"
      if curl --fail --silent --show-error --location --head \
        --dump-header "${head_json}" \
        --output /dev/null \
        "${url}"; then
        if public_head_matches_expected "${url}" "${expected_sha}" "${expected_size}" "${label}"; then
          return 0
        fi

        exit 1
      else
        last_error="HEAD request failed"
      fi

      if ((attempt < public_verify_attempts)); then
        echo "public Runtime release ${label} is not ready on attempt ${attempt}/${public_verify_attempts}: ${last_error}; retrying in ${public_verify_sleep_seconds}s: ${url}" >&2
        if ((public_verify_sleep_seconds > 0)); then
          sleep "${public_verify_sleep_seconds}"
        fi
      fi
    done

    if [[ "${last_error}" == HEAD\ request\ failed ]]; then
      echo "failed to verify public Runtime release ${label}: ${url}" >&2
    fi
    exit 1
  }

  upload_object "${asset_path}" "${asset_key}" "${asset_sha}" "${asset_size}" "application/gzip"
  upload_object "${checksum_path}" "${checksum_key}" "${checksum_sha}" "${checksum_size}" "text/plain"
  upload_object "${manifest_path}" "${manifest_key}" "${manifest_sha}" "${manifest_size}" "application/json"

  if [[ "${skip_public_verification}" != "true" ]]; then
    verify_public_head_metadata "${asset_url}" "${asset_sha}" "${asset_size}" "asset ${asset_name}"
    verify_public_head_metadata "${checksum_url}" "${checksum_sha}" "${checksum_size}" "checksum ${checksum_name}"
    verify_public_head_metadata "${manifest_url}" "${manifest_sha}" "${manifest_size}" "manifest ${manifest_name}"
  fi
fi

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    echo "asset_url=${asset_url}"
    echo "checksum_url=${checksum_url}"
    echo "manifest_url=${manifest_url}"
    echo "manifest_path=${manifest_path}"
    echo "asset_key=${asset_key}"
    echo "checksum_key=${checksum_key}"
    echo "manifest_key=${manifest_key}"
  } >>"${GITHUB_OUTPUT}"
fi
