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
    echo "same Runtime packaging inputs produced different archive bytes" >&2
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
}

linux_work_dir="${tmp_root}/linux-work"
linux_target="x86_64-unknown-linux-gnu"
linux_slug="linux-x64"
linux_asset_name="skenion-runtime-v${version}-${linux_slug}.tar.gz"
package_twice "${linux_work_dir}" "${linux_target}" "skenion-runtime" "${linux_asset_name}"

python3 - "${tmp_root}/dist-a/${linux_asset_name}" "${version}" "${linux_target}" "${linux_slug}" <<'PY'
import gzip
import sys
import tarfile

asset_path, version, target, platform_slug = sys.argv[1:]
package_name = f"skenion-runtime-v{version}-{platform_slug}"
expected_names = [
    package_name,
    f"{package_name}/README.txt",
    f"{package_name}/skenion-runtime",
]

with open(asset_path, "rb") as fh:
    header = fh.read(10)
assert header[4:8] == b"\x00\x00\x00\x00", header

with gzip.open(asset_path, "rb") as gz:
    with tarfile.open(fileobj=gz, mode="r:") as archive:
        members = archive.getmembers()
        assert [member.name for member in members] == expected_names
        assert [member.mtime for member in members] == [0, 0, 0]
        assert [member.uid for member in members] == [0, 0, 0]
        assert [member.gid for member in members] == [0, 0, 0]
        assert [member.uname for member in members] == ["", "", ""]
        assert [member.gname for member in members] == ["", "", ""]
        assert [oct(member.mode) for member in members] == ["0o755", "0o644", "0o755"]
        assert members[0].isdir()
        assert members[1].isfile()
        assert members[2].isfile()
        readme = archive.extractfile(members[1]).read().decode("utf-8")
        assert readme == f"skenion-runtime {version}\nPlatform: {platform_slug}\nTarget: {target}\n"
PY

rm -rf "${tmp_root}/dist-a" "${tmp_root}/dist-b"

windows_work_dir="${tmp_root}/windows-work"
windows_target="x86_64-pc-windows-msvc"
windows_slug="windows-x64"
windows_asset_name="skenion-runtime-v${version}-${windows_slug}.zip"
package_twice "${windows_work_dir}" "${windows_target}" "skenion-runtime.exe" "${windows_asset_name}"

python3 - "${tmp_root}/dist-a/${windows_asset_name}" "${version}" "${windows_target}" "${windows_slug}" <<'PY'
import sys
import zipfile

asset_path, version, target, platform_slug = sys.argv[1:]
package_name = f"skenion-runtime-v{version}-{platform_slug}"
expected_names = [
    f"{package_name}/",
    f"{package_name}/README.txt",
    f"{package_name}/skenion-runtime.exe",
]

with zipfile.ZipFile(asset_path) as archive:
    members = archive.infolist()
    assert [member.filename for member in members] == expected_names
    assert [member.date_time for member in members] == [(1980, 1, 1, 0, 0, 0)] * 3
    assert [oct(member.external_attr >> 16) for member in members] == ["0o755", "0o644", "0o755"]
    readme = archive.read(f"{package_name}/README.txt").decode("utf-8")
    assert readme == f"skenion-runtime {version}\nPlatform: {platform_slug}\nTarget: {target}\n"
PY

echo "Runtime asset packaging validation passed."
