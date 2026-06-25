#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runtime_manifest="${repo_root}/Cargo.toml"
default_contracts_rust_path="${repo_root}/../skenion-contracts/packages/rust"
legacy_contracts_rust_path="${repo_root}/../Skenion-contracts/packages/rust"

if [[ -n "${SKENION_CONTRACTS_RUST_PATH:-}" ]]; then
  contracts_rust_path="${SKENION_CONTRACTS_RUST_PATH}"
elif [[ -f "${default_contracts_rust_path}/Cargo.toml" ]]; then
  contracts_rust_path="${default_contracts_rust_path}"
elif [[ -f "${legacy_contracts_rust_path}/Cargo.toml" ]]; then
  contracts_rust_path="${legacy_contracts_rust_path}"
else
  contracts_rust_path="${default_contracts_rust_path}"
fi

usage() {
  cat <<'EOF'
usage: scripts/check-local-contracts-integration.sh [cargo-subcommand-and-args...]

Validates Runtime against a local skenion-contracts Rust crate by creating a
temporary Cargo [patch.crates-io] config. This is a developer-only integration
path; release workflows must consume the released crates.io dependency.

Environment:
  SKENION_CONTRACTS_RUST_PATH  Path to the local skenion-contracts Rust crate.
                              Defaults to ../skenion-contracts/packages/rust,
                              with a fallback to the historical
                              ../Skenion-contracts/packages/rust checkout name.
  CARGO                       Cargo binary to use. Defaults to cargo or
                              ~/.cargo/bin/cargo.

With no cargo arguments, runs:
  cargo test --all-targets --all-features

The helper restores Cargo.lock after Cargo resolves the temporary local patch.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

die() {
  echo "error: $*" >&2
  exit 1
}

command -v python3 >/dev/null 2>&1 || die "python3 is required."

if [[ -n "${CARGO:-}" ]]; then
  cargo_bin="${CARGO}"
elif command -v cargo >/dev/null 2>&1; then
  cargo_bin="$(command -v cargo)"
elif [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
  cargo_bin="${HOME}/.cargo/bin/cargo"
else
  die "cargo is required. Set CARGO or install cargo on PATH."
fi

tmpdir="$(mktemp -d)"
lockfile="${repo_root}/Cargo.lock"
lock_backup="${tmpdir}/Cargo.lock.backup"
lock_was_present=false

if [[ -f "${lockfile}" ]]; then
  cp "${lockfile}" "${lock_backup}"
  lock_was_present=true
fi

restore_lockfile() {
  local restored=false

  if [[ "${lock_was_present}" == "true" ]]; then
    if [[ ! -f "${lockfile}" ]] || ! cmp -s "${lock_backup}" "${lockfile}"; then
      cp "${lock_backup}" "${lockfile}"
      restored=true
    fi
  elif [[ -f "${lockfile}" ]]; then
    rm -f "${lockfile}"
    restored=true
  fi

  if [[ "${restored}" == "true" ]]; then
    echo "Restored Cargo.lock after temporary local Contracts patch resolution." >&2
  fi
}

cleanup() {
  local status=$?
  restore_lockfile
  rm -rf "${tmpdir}"
  exit "${status}"
}

trap cleanup EXIT

validation_json="${tmpdir}/local-contracts-validation.json"
config_file="${tmpdir}/cargo-config.toml"
metadata_json="${tmpdir}/cargo-metadata.json"

python3 - "${runtime_manifest}" "${contracts_rust_path}" "${validation_json}" <<'PY'
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    tomllib = None

SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def resolve_existing(path: Path, label: str) -> Path:
    try:
        return path.expanduser().resolve(strict=True)
    except FileNotFoundError:
        fail(f"{label} does not exist: {path}")


def parse_quoted_value(line: str):
    match = re.search(r'=\s*"([^"]*)"', line)
    return match.group(1) if match else None


def parse_inline_table(value: str) -> dict:
    return {
        key: val
        for key, val in re.findall(r'([A-Za-z0-9_-]+)\s*=\s*"([^"]*)"', value)
    }


def read_minimal_manifest(path: Path) -> dict:
    manifest = {"package": {}, "dependencies": {}}
    section = ""
    dependency_table = False

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        header = re.match(r"^\[([^\]]+)\]$", line)
        if header:
            section = header.group(1).strip()
            dependency_table = section in {
                "dependencies.skenion-contracts",
                'dependencies."skenion-contracts"',
            }
            if dependency_table:
                manifest["dependencies"].setdefault("skenion-contracts", {})
            continue

        if section == "package" and re.match(r"^(name|version)\s*=", line):
            key = line.split("=", 1)[0].strip()
            value = parse_quoted_value(line)
            if value is not None:
                manifest["package"][key] = value
            continue

        if section == "dependencies" and line.startswith("skenion-contracts"):
            value = line.split("=", 1)[1].strip()
            if value.startswith('"'):
                parsed = re.match(r'^"([^"]+)"', value)
                if parsed:
                    manifest["dependencies"]["skenion-contracts"] = parsed.group(1)
            elif value.startswith("{"):
                manifest["dependencies"]["skenion-contracts"] = parse_inline_table(value)
            continue

        if dependency_table and re.match(r"^(version|path|git|branch|tag|rev)\s*=", line):
            key = line.split("=", 1)[0].strip()
            value = parse_quoted_value(line)
            if value is not None:
                manifest["dependencies"]["skenion-contracts"][key] = value

    return manifest


def read_toml(path: Path, label: str) -> dict:
    if not path.is_file():
        fail(f"{label} was not found: {path}")
    if tomllib is not None:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    return read_minimal_manifest(path)


def parse_v0_line(version: str, label: str) -> str:
    match = SEMVER.fullmatch(version)
    if not match:
        fail(f"{label} must be exact SemVer x.y.z; got {version!r}.")
    major, minor, _patch = (int(part) for part in version.split("."))
    if major != 0:
        fail(f"{label} must stay on the v0 Contracts line; got {version!r}.")
    return f"0.{minor}"


def runtime_contracts_dependency(manifest: dict) -> str:
    dependency = manifest.get("dependencies", {}).get("skenion-contracts")
    if dependency is None:
        fail("Runtime Cargo.toml must declare a skenion-contracts dependency.")
    if isinstance(dependency, str):
        version = dependency.strip()
    elif isinstance(dependency, dict):
        forbidden = sorted(
            key
            for key in ("path", "git", "branch", "tag", "rev")
            if key in dependency
        )
        if forbidden:
            fail(
                "Runtime Cargo.toml must remain registry-first; "
                f"remove skenion-contracts keys {forbidden}."
            )
        version = str(dependency.get("version", "")).strip()
    else:
        fail("skenion-contracts dependency must be a string or table dependency.")
    if not version:
        fail("skenion-contracts dependency must declare a version.")
    return version


runtime_manifest = resolve_existing(Path(sys.argv[1]), "Runtime Cargo.toml")
contracts_path = Path(sys.argv[2]).expanduser()
if not contracts_path.is_absolute():
    contracts_path = Path.cwd() / contracts_path
if not (contracts_path / "Cargo.toml").is_file() and (
    contracts_path / "packages/rust/Cargo.toml"
).is_file():
    contracts_path = contracts_path / "packages/rust"
contracts_path = resolve_existing(contracts_path, "local skenion-contracts Rust path")
output_path = Path(sys.argv[3])

runtime = read_toml(runtime_manifest, "Runtime Cargo.toml")
expected_version = runtime_contracts_dependency(runtime)
expected_line = parse_v0_line(expected_version, "Runtime skenion-contracts dependency")

contracts_manifest = read_toml(contracts_path / "Cargo.toml", "Contracts Cargo.toml")
package = contracts_manifest.get("package", {})
if package.get("name") != "skenion-contracts":
    fail(
        "SKENION_CONTRACTS_RUST_PATH must point to the skenion-contracts Rust "
        f"crate; found package {package.get('name')!r}."
    )

actual_version = str(package.get("version", "")).strip()
actual_line = parse_v0_line(actual_version, "local skenion-contracts package version")
if actual_version != expected_version:
    fail(
        "local skenion-contracts version mismatch: Runtime requires "
        f"{expected_version} (line {expected_line}), but {contracts_path} "
        f"declares {actual_version} (line {actual_line})."
    )
if actual_line != expected_line:
    fail(
        "local skenion-contracts line mismatch: Runtime requires "
        f"{expected_line}, but {contracts_path} declares {actual_line}."
    )

git_evidence = None
if shutil.which("git"):
    commit = subprocess.run(
        ["git", "-C", str(contracts_path), "rev-parse", "--verify", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    branch = subprocess.run(
        ["git", "-C", str(contracts_path), "branch", "--show-current"],
        check=False,
        capture_output=True,
        text=True,
    )
    if commit.returncode == 0:
        branch_name = branch.stdout.strip() if branch.returncode == 0 else ""
        git_evidence = {
            "commit": commit.stdout.strip(),
            "branch": branch_name or "detached",
        }

validation = {
    "runtime_manifest": str(runtime_manifest),
    "contracts_path": str(contracts_path),
    "expected_version": expected_version,
    "expected_line": expected_line,
    "expected_range": f">={expected_line}.0 <0.{int(expected_line.split('.')[1]) + 1}.0",
    "git_evidence": git_evidence,
}
output_path.write_text(json.dumps(validation, indent=2) + "\n", encoding="utf-8")

print(
    "Runtime requires skenion-contracts "
    f"{expected_version} on Contracts line {expected_line}.",
    file=sys.stderr,
)
print(
    "Local skenion-contracts crate matches at "
    f"{contracts_path}.",
    file=sys.stderr,
)
if git_evidence:
    print(
        "Local Contracts checkout evidence: "
        f"{git_evidence['branch']} @ {git_evidence['commit']}.",
        file=sys.stderr,
    )
PY

python3 - "${validation_json}" "${config_file}" <<'PY'
import json
import sys
from pathlib import Path

validation = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
contracts_path = validation["contracts_path"].replace("\\", "\\\\").replace('"', '\\"')
Path(sys.argv[2]).write_text(
    "[patch.crates-io]\n"
    f'skenion-contracts = {{ path = "{contracts_path}" }}\n',
    encoding="utf-8",
)
PY

(
  cd "${repo_root}"
  "${cargo_bin}" --config "${config_file}" metadata --format-version 1
) >"${metadata_json}"

python3 - "${validation_json}" "${metadata_json}" <<'PY'
import json
import sys
from pathlib import Path

validation = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
metadata = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))

expected_version = validation["expected_version"]
expected_manifest = (Path(validation["contracts_path"]) / "Cargo.toml").resolve()
matches = [
    package
    for package in metadata.get("packages", [])
    if package.get("name") == "skenion-contracts"
    and package.get("version") == expected_version
]

if len(matches) != 1:
    print(
        "error: Cargo metadata did not resolve exactly one "
        f"skenion-contracts {expected_version}; found {len(matches)}.",
        file=sys.stderr,
    )
    raise SystemExit(1)

actual_manifest = Path(matches[0]["manifest_path"]).resolve()
if actual_manifest != expected_manifest:
    print(
        "error: Cargo did not use the local skenion-contracts patch; "
        f"resolved {actual_manifest}, expected {expected_manifest}.",
        file=sys.stderr,
    )
    raise SystemExit(1)

print(
    f"Cargo resolved skenion-contracts {expected_version} from {expected_manifest.parent}.",
    file=sys.stderr,
)
PY

if [[ $# -eq 0 ]]; then
  cargo_args=(test --all-targets --all-features)
else
  cargo_args=("$@")
fi

{
  printf 'Running local Contracts integration command: %q' "${cargo_bin}"
  printf ' %q' --config "${config_file}" "${cargo_args[@]}"
  printf '\n'
} >&2

(
  cd "${repo_root}"
  "${cargo_bin}" --config "${config_file}" "${cargo_args[@]}"
)
