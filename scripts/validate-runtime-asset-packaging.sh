#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
packager="${repo_root}/scripts/package-runtime-asset.sh"
tmp_root="$(mktemp -d)"
version="9.8.7"

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

validate_checksums_match() {
  local asset_a="$1"
  local asset_b="$2"
  local checksum_a="${asset_a}.sha256"
  local checksum_b="${asset_b}.sha256"
  local expected_sha

  if ! cmp -s "${asset_a}" "${asset_b}"; then
    echo "same Runtime packaging inputs produced different binary bytes" >&2
    echo "first sha256=$(sha256_file "${asset_a}")" >&2
    echo "second sha256=$(sha256_file "${asset_b}")" >&2
    exit 1
  fi

  expected_sha="$(sha256_file "${asset_a}")"
  if [[ "$(awk '{print $1; exit}' "${checksum_a}")" != "${expected_sha}" ]]; then
    echo "first checksum file does not match packaged asset" >&2
    exit 1
  fi
  if [[ "$(awk '{print $1; exit}' "${checksum_b}")" != "${expected_sha}" ]]; then
    echo "second checksum file does not match packaged asset" >&2
    exit 1
  fi
}

package_twice() {
  local work_dir="$1"
  local target="$2"
  local binary_name="$3"
  local asset_name="$4"
  local binary_dir="${work_dir}/target/${target}/release"

  mkdir -p "${binary_dir}"
  printf 'deterministic runtime binary bytes\n' >"${binary_dir}/${binary_name}"
  chmod 600 "${binary_dir}/${binary_name}"

  (
    cd "${work_dir}"
    "${packager}" "${target}" "${version}" "${tmp_root}/dist-a" >/dev/null
  )

  sleep 1
  touch "${binary_dir}/${binary_name}"
  (
    cd "${work_dir}"
    "${packager}" "${target}" "${version}" "${tmp_root}/dist-b" >/dev/null
  )

  validate_checksums_match "${tmp_root}/dist-a/${asset_name}" "${tmp_root}/dist-b/${asset_name}"
  if ! cmp -s "${binary_dir}/${binary_name}" "${tmp_root}/dist-a/${asset_name}"; then
    echo "packaged Runtime asset does not match built executable bytes" >&2
    exit 1
  fi
}

linux_work_dir="${tmp_root}/linux-work"
linux_target="x86_64-unknown-linux-gnu"
linux_slug="linux-x64"
linux_asset_name="skenion-runtime-v${version}-${linux_slug}"
package_twice "${linux_work_dir}" "${linux_target}" "skenion-runtime" "${linux_asset_name}"

rm -rf "${tmp_root}/dist-a" "${tmp_root}/dist-b"

windows_work_dir="${tmp_root}/windows-work"
windows_target="x86_64-pc-windows-msvc"
windows_slug="windows-x64"
windows_asset_name="skenion-runtime-v${version}-${windows_slug}.exe"
package_twice "${windows_work_dir}" "${windows_target}" "skenion-runtime.exe" "${windows_asset_name}"

echo "Runtime asset packaging validation passed."
