#!/usr/bin/env bash
set -euo pipefail

dry_run=false
delete_github_manifest_assets=false
body_file=""
output_file=""

while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --dry-run)
      dry_run=true
      ;;
    --delete-github-manifest-assets)
      delete_github_manifest_assets=true
      ;;
    --body-file)
      body_file="$2"
      shift
      ;;
    --output)
      output_file="$2"
      shift
      ;;
    *)
      echo "unknown option: $1" >&2
      exit 2
      ;;
  esac
  shift
done

if [[ $# -ne 2 ]]; then
  echo "usage: $0 [--dry-run] [--delete-github-manifest-assets] [--body-file <path>] [--output <path>] <release-tag> <runtime-version>" >&2
  exit 2
fi

release_tag="$1"
version="$2"

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "required environment variable is missing: ${name}" >&2
    exit 1
  fi
}

require_python() {
  if command -v python3 >/dev/null 2>&1; then
    command -v python3
  elif command -v python >/dev/null 2>&1; then
    command -v python
  else
    echo "python3 or python is required for Runtime release body generation." >&2
    exit 1
  fi
}

if [[ ! "${release_tag}" =~ ^v${version//./\\.}$ ]]; then
  echo "release tag must match runtime version, got tag=${release_tag} version=${version}" >&2
  exit 1
fi

require_env SKENION_RELEASE_PUBLIC_BASE_URL
python_bin="$(require_python)"

if [[ "${dry_run}" != "true" ]]; then
  require_env GH_TOKEN
  require_env GITHUB_REPOSITORY
  if ! command -v gh >/dev/null 2>&1; then
    echo "gh CLI is required to update GitHub Release notes." >&2
    exit 1
  fi
fi

public_base="${SKENION_RELEASE_PUBLIC_BASE_URL%/}"
section_path="$(mktemp)"
body_path="$(mktemp)"
updated_body_path="$(mktemp)"
cleanup() {
  rm -f "${section_path}" "${body_path}" "${updated_body_path}"
}
trap cleanup EXIT

write_download_section() {
  "${python_bin}" - "${section_path}" "${public_base}" "${release_tag}" "${version}" <<'PY'
import sys

output_path, public_base, release_tag, version = sys.argv[1:]
targets = [
    ("macOS Apple Silicon", "macos-apple-silicon", "", "release"),
    ("macOS Intel", "macos-intel", "", "release"),
    ("Windows x64", "windows-x64", ".exe", "release"),
    ("Windows ARM64", "windows-arm64", ".exe", "preview"),
    ("Linux x64", "linux-x64", "", "release"),
    ("Linux ARM64", "linux-arm64", "", "preview"),
]

def binary_url(platform_slug: str, extension: str) -> str:
    filename = f"skenion-runtime-v{version}-{platform_slug}{extension}"
    return f"{public_base}/skenion-runtime/{release_tag}/{platform_slug}/{filename}"

lines = [
    "<!-- skenion-runtime-downloads:start -->",
    "### Runtime Downloads",
    "",
    "Runtime binaries are served from DSUB S3. Use the SHA-256 file next to each binary to verify downloads.",
    "",
    "| Platform | Tier | Binary | SHA-256 |",
    "| --- | --- | --- | --- |",
]

for platform, platform_slug, extension, tier in targets:
    binary = binary_url(platform_slug, extension)
    checksum = f"{binary}.sha256"
    lines.append(
        f"| {platform} | {tier} | [binary]({binary}) | [sha256]({checksum}) |"
    )

lines.extend(["", "<!-- skenion-runtime-downloads:end -->", ""])

with open(output_path, "w", encoding="utf-8") as fh:
    fh.write("\n".join(lines))
PY
}

read_release_body() {
  if [[ -n "${body_file}" ]]; then
    cp "${body_file}" "${body_path}"
  else
    gh release view "${release_tag}" --json body --jq '.body' >"${body_path}"
  fi
}

write_updated_body() {
  "${python_bin}" - "${body_path}" "${section_path}" "${updated_body_path}" <<'PY'
import re
import sys
from pathlib import Path

body_path, section_path, output_path = map(Path, sys.argv[1:])
body = body_path.read_text(encoding="utf-8")
section = section_path.read_text(encoding="utf-8")

pattern = re.compile(
    r"\n*<!-- skenion-runtime-downloads:start -->.*?<!-- skenion-runtime-downloads:end -->\n*",
    re.DOTALL,
)

clean = pattern.sub("\n", body).rstrip()
updated = f"{clean}\n\n{section}" if clean else section
Path(output_path).write_text(updated, encoding="utf-8")
PY
}

delete_manifest_assets() {
  local asset
  local pattern_binary

  pattern_binary="skenion-runtime-v${version}-*.manifest.json"
  while IFS= read -r asset; do
    # This pattern intentionally uses the platform wildcard to remove all per-platform manifest assets.
    # shellcheck disable=SC2254
    case "${asset}" in
      ${pattern_binary})
        gh release delete-asset "${release_tag}" "${asset}" --yes
        ;;
    esac
  done < <(gh release view "${release_tag}" --json assets --jq '.assets[].name')
}

write_download_section
read_release_body
write_updated_body

if [[ -n "${output_file}" ]]; then
  cp "${updated_body_path}" "${output_file}"
elif [[ "${dry_run}" == "true" ]]; then
  cat "${updated_body_path}"
fi

if [[ "${dry_run}" == "true" ]]; then
  exit 0
fi

gh release edit "${release_tag}" --notes-file "${updated_body_path}"
if [[ "${delete_github_manifest_assets}" == "true" ]]; then
  delete_manifest_assets
fi
