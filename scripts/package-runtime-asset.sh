#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target-triple> <version> <output-dir>" >&2
  exit 2
fi

target="$1"
version="$2"
output_dir="$3"

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

runtime_asset_filename() {
  case "$1" in
    *windows*)
      printf 'skenion-runtime-v%s-%s.exe' "$2" "$3"
      ;;
    *)
      printf 'skenion-runtime-v%s-%s' "$2" "$3"
      ;;
  esac
}

binary_name="skenion-runtime"
if [[ "${target}" == *windows* ]]; then
  binary_name="skenion-runtime.exe"
fi

binary_path="target/${target}/release/${binary_name}"
if [[ ! -f "${binary_path}" ]]; then
  echo "runtime binary not found: ${binary_path}" >&2
  exit 1
fi

platform_slug="$(runtime_platform_slug "${target}")"
asset_name="$(runtime_asset_filename "${target}" "${version}" "${platform_slug}")"
asset_path="${output_dir}/${asset_name}"
checksum_path="${asset_path}.sha256"

mkdir -p "${output_dir}"
cp "${binary_path}" "${asset_path}"

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
    echo "platform_slug=${platform_slug}"
    echo "binary_format=raw-binary"
  } >>"${GITHUB_OUTPUT}"
fi

echo "packaged ${asset_path}"
echo "wrote ${checksum_path}"
