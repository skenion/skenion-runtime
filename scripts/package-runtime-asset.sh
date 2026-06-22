#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target-triple> <version> <output-dir>" >&2
  exit 2
fi

target="$1"
version="$2"
output_dir="$3"

binary_name="skenion-runtime"
if [[ "${target}" == *windows* ]]; then
  binary_name="skenion-runtime.exe"
fi

binary_path="target/${target}/release/${binary_name}"
if [[ ! -f "${binary_path}" ]]; then
  echo "runtime binary not found: ${binary_path}" >&2
  exit 1
fi

asset_name="skenion-runtime-v${version}-${target}.tar.gz"
asset_path="${output_dir}/${asset_name}"
checksum_path="${asset_path}.sha256"
staging_dir="$(mktemp -d)"
package_dir="${staging_dir}/skenion-runtime-v${version}-${target}"

cleanup() {
  rm -rf "${staging_dir}"
}
trap cleanup EXIT

mkdir -p "${output_dir}" "${package_dir}"
cp "${binary_path}" "${package_dir}/${binary_name}"
chmod 755 "${package_dir}/${binary_name}" 2>/dev/null || true
printf "skenion-runtime %s\nTarget: %s\n" "${version}" "${target}" >"${package_dir}/README.txt"

tar -czf "${asset_path}" -C "${staging_dir}" "skenion-runtime-v${version}-${target}"

if command -v sha256sum >/dev/null 2>&1; then
  (
    cd "${output_dir}"
    sha256sum "${asset_name}" >"${asset_name}.sha256"
  )
elif command -v shasum >/dev/null 2>&1; then
  (
    cd "${output_dir}"
    shasum -a 256 "${asset_name}" >"${asset_name}.sha256"
  )
else
  echo "no sha256 checksum tool found" >&2
  exit 1
fi

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    echo "asset_name=${asset_name}"
    echo "asset_path=${asset_path}"
    echo "checksum_path=${checksum_path}"
  } >>"${GITHUB_OUTPUT}"
fi

echo "packaged ${asset_path}"
echo "wrote ${checksum_path}"
