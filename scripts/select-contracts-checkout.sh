#!/usr/bin/env bash
set -euo pipefail

runtime_manifest="${RUNTIME_MANIFEST:-Cargo.toml}"
contracts_checkout="${CONTRACTS_CHECKOUT:-.deps/skenion-contracts}"
candidate_branch="${CONTRACTS_BRANCH:-${GITHUB_HEAD_REF:-}}"

ci_error() {
  echo "::error::$*" >&2
}

read_required_version() {
  python3 - "$runtime_manifest" <<'PY'
import re
import sys
from pathlib import Path

manifest = Path(sys.argv[1])
section = ""
for line in manifest.read_text(encoding="utf-8").splitlines():
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        section = stripped
        continue
    if section == "[dependencies]" and re.match(r"^\s*skenion-contracts\s*=", line):
        match = re.search(r'version\s*=\s*"([^"]+)"', line)
        if not match:
            raise SystemExit("skenion-contracts dependency must declare a version")
        print(match.group(1))
        raise SystemExit(0)

raise SystemExit("skenion-contracts dependency was not found")
PY
}

read_contracts_version() {
  local manifest="$1"
  python3 - "$manifest" <<'PY'
import re
import sys
from pathlib import Path

manifest = Path(sys.argv[1])
section = ""
for line in manifest.read_text(encoding="utf-8").splitlines():
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        section = stripped
        continue
    if section == "[package]" and re.match(r"^\s*version\s*=", line):
        match = re.search(r'"([^"]+)"', line)
        if not match:
            raise SystemExit("Contracts package version line is malformed")
        print(match.group(1))
        raise SystemExit(0)

raise SystemExit("Contracts package version was not found")
PY
}

required_version="$(read_required_version)"
required_tag="skenion-contracts-v${required_version}"

if [[ ! -d "${contracts_checkout}/.git" ]]; then
  ci_error "Contracts checkout '${contracts_checkout}' is not a git repository."
  exit 1
fi

cd "${contracts_checkout}"

selected_ref=""
if [[ -n "${candidate_branch}" ]] && git ls-remote --exit-code origin "refs/heads/${candidate_branch}" >/dev/null 2>&1; then
  git fetch --depth=1 origin "+refs/heads/${candidate_branch}:refs/remotes/origin/${candidate_branch}"
  git switch --detach "refs/remotes/origin/${candidate_branch}"
  selected_ref="branch ${candidate_branch}"
elif git ls-remote --exit-code origin "refs/tags/${required_tag}" >/dev/null 2>&1; then
  git fetch --depth=1 origin "+refs/tags/${required_tag}:refs/tags/${required_tag}"
  git switch --detach "${required_tag}"
  selected_ref="tag ${required_tag}"
else
  if [[ -n "${candidate_branch}" ]]; then
    ci_error "No Contracts branch '${candidate_branch}' or tag '${required_tag}' exists."
  else
    ci_error "No Contracts tag '${required_tag}' exists."
  fi
  ci_error "Runtime requires skenion-contracts ${required_version}; refusing to fall back to main."
  exit 1
fi

actual_version="$(read_contracts_version packages/rust/Cargo.toml)"
if [[ "${actual_version}" != "${required_version}" ]]; then
  ci_error "Selected Contracts ${selected_ref} has version ${actual_version}, but Runtime requires ${required_version}."
  exit 1
fi

echo "Selected Contracts ${selected_ref} for skenion-contracts ${required_version}."
