#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target-triple> <version> <output-dir>" >&2
  exit 2
fi

target="$1"
version="$2"
output_dir="$3"

find_python() {
  if command -v python3 >/dev/null 2>&1; then
    command -v python3
  elif command -v python >/dev/null 2>&1; then
    command -v python
  else
    echo "python3 or python is required for deterministic Runtime asset packaging." >&2
    exit 1
  fi
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

asset_name="skenion-runtime-v${version}-${target}.tar.gz"
asset_path="${output_dir}/${asset_name}"
checksum_path="${asset_path}.sha256"
python_bin="$(find_python)"

mkdir -p "${output_dir}"

"${python_bin}" - "${binary_path}" "${asset_path}" "${version}" "${target}" "${binary_name}" <<'PY'
import gzip
import io
import os
import sys
import tarfile

binary_path, asset_path, version, target, binary_name = sys.argv[1:]
package_name = f"skenion-runtime-v{version}-{target}"
readme_bytes = f"skenion-runtime {version}\nTarget: {target}\n".encode("utf-8")


def tar_info(name, size, mode, type_=tarfile.REGTYPE):
    info = tarfile.TarInfo(name)
    info.size = size
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    info.type = type_
    return info


with open(asset_path, "wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            directory = tar_info(package_name, 0, 0o755, tarfile.DIRTYPE)
            archive.addfile(directory)

            readme_name = f"{package_name}/README.txt"
            archive.addfile(tar_info(readme_name, len(readme_bytes), 0o644), io.BytesIO(readme_bytes))

            binary_name_in_archive = f"{package_name}/{binary_name}"
            binary_size = os.path.getsize(binary_path)
            with open(binary_path, "rb") as binary:
                archive.addfile(tar_info(binary_name_in_archive, binary_size, 0o755), binary)
PY

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
