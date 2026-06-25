#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
packager="${repo_root}/scripts/package-runtime-asset.sh"
tmp_root="$(mktemp -d)"
target="x86_64-unknown-linux-gnu"
version="9.8.7"
asset_name="skenion-runtime-v${version}-${target}.tar.gz"

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

work_dir="${tmp_root}/work"
binary_dir="${work_dir}/target/${target}/release"
mkdir -p "${binary_dir}"
printf 'deterministic runtime binary bytes\n' >"${binary_dir}/skenion-runtime"
chmod 600 "${binary_dir}/skenion-runtime"

(
  cd "${work_dir}"
  "${packager}" "${target}" "${version}" "${tmp_root}/dist-a" >/dev/null
)

sleep 1
touch "${binary_dir}/skenion-runtime"
(
  cd "${work_dir}"
  "${packager}" "${target}" "${version}" "${tmp_root}/dist-b" >/dev/null
)

asset_a="${tmp_root}/dist-a/${asset_name}"
asset_b="${tmp_root}/dist-b/${asset_name}"
checksum_a="${asset_a}.sha256"
checksum_b="${asset_b}.sha256"

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

python3 - "${asset_a}" "${version}" "${target}" <<'PY'
import gzip
import sys
import tarfile

asset_path, version, target = sys.argv[1:]
package_name = f"skenion-runtime-v{version}-{target}"
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
        assert readme == f"skenion-runtime {version}\nTarget: {target}\n"
PY

echo "Runtime asset packaging validation passed."
