#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="${repo_root}/.github/workflows/publish.yml"

python3 - "${workflow}" <<'PY'
import re
import sys
from pathlib import Path

workflow_path = Path(sys.argv[1])
repo_root = workflow_path.parents[2]
lines = workflow_path.read_text(encoding="utf-8").splitlines()
workflow_text = "\n".join(lines)
ci_text = (repo_root / ".github/workflows/ci.yml").read_text(encoding="utf-8")
release_downloads_script = (repo_root / "scripts/update-runtime-release-downloads.sh").read_text(encoding="utf-8")

jobs = {}
current = None
for line in lines:
    match = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
    if match:
        current = match.group(1)
        jobs[current] = []
    if current is not None:
        jobs[current].append(line)


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


required_jobs = {"runtime-assets", "release-downloads"}
missing = required_jobs - set(jobs)
if missing:
    fail(f"publish workflow is missing required jobs: {sorted(missing)}")

for forbidden in ("actions/upload-artifact@", "actions/download-artifact@"):
    if forbidden in workflow_text:
        fail(f"publish workflow must not use GitHub Actions artifacts for Runtime release handoff; found {forbidden!r}")

for forbidden in (
    "scripts/check-local-contracts-integration.sh",
    "SKENION_CONTRACTS_RUST_PATH",
    "patch.crates-io",
):
    if forbidden in workflow_text:
        fail(f"publish workflow must not use developer-only local Contracts integration; found {forbidden!r}")

runtime_assets = "\n".join(jobs["runtime-assets"])
if "scripts/check-runtime-asset-s3-existing.sh" not in runtime_assets:
    fail("runtime-assets job must check DSUB S3 before building release binaries")
if "cargo build --release" not in runtime_assets:
    fail("runtime-assets job must build the release binary exactly once per target")
if "scripts/package-runtime-asset.sh" not in runtime_assets:
    fail("runtime-assets job must package the release binary exactly once per target")
if "scripts/publish-runtime-asset-s3.sh" not in runtime_assets:
    fail("runtime-assets job must publish the package produced in the same job attempt")
if "gh release upload" in runtime_assets:
    fail("runtime-assets job must not upload metadata-only manifest assets to GitHub Release")
if "GitHub Release manifest asset" in runtime_assets:
    fail("runtime-assets summary must not advertise metadata-only GitHub Release manifest assets")

if runtime_assets.count("cargo build --release") != 1:
    fail("runtime-assets job must contain exactly one release cargo build command")
if runtime_assets.count("scripts/package-runtime-asset.sh") != 1:
    fail("runtime-assets job must contain exactly one package-runtime-asset.sh invocation")
if runtime_assets.count("scripts/check-runtime-asset-s3-existing.sh") != 1:
    fail("runtime-assets job must contain exactly one pre-build DSUB S3 existence check")

existing_index = runtime_assets.index("scripts/check-runtime-asset-s3-existing.sh")
build_index = runtime_assets.index("cargo build --release")
if existing_index > build_index:
    fail("runtime-assets job must check S3 before building")

if runtime_assets.count("if: steps.existing.outputs.exists != 'true'") < 8:
    fail("runtime-assets build/package/publish steps must be gated by the S3 existence check")
if "--skip-public-verification" not in runtime_assets:
    fail("runtime-assets publish step must not block on CDN public verification")

for required_slug in (
    "macos-apple-silicon",
    "macos-intel",
    "windows-x64",
    "windows-arm64",
    "linux-x64",
    "linux-arm64",
):
    if required_slug not in release_downloads_script:
        fail(f"release download generator must expose public platform slug {required_slug!r}")

for forbidden_public_token in (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
):
    if forbidden_public_token in release_downloads_script:
        fail(f"release download generator must not expose Rust target triple {forbidden_public_token!r}")

for public_fixture_text in (workflow_text, ci_text, release_downloads_script):
    if re.search(r"skenion-runtime/v[^\n]*unknown-linux-gnu", public_fixture_text):
        fail("public Runtime release links must not contain unknown-linux-gnu target triples")
    if re.search(r"skenion-runtime-v[^\n]*unknown-linux-gnu", public_fixture_text):
        fail("public Runtime release filenames must not contain unknown-linux-gnu target triples")
    if re.search(r"windows-(?:x64|arm64)[^\n]*\.tar\.gz", public_fixture_text):
        fail("Windows Runtime public archives must use .zip, not .tar.gz")

release_downloads = "\n".join(jobs["release-downloads"])
if "scripts/update-runtime-release-downloads.sh" not in release_downloads:
    fail("release-downloads job must update GitHub Release notes with DSUB S3 download links")
if "--delete-github-manifest-assets" not in release_downloads:
    fail("release-downloads job must remove old metadata-only GitHub Release manifest assets")
if "ref: main" not in release_downloads:
    fail("release-downloads job must checkout main so workflow_dispatch can repair older release tags")
if "gh release upload" in release_downloads:
    fail("release-downloads job must not upload release assets")

for job_name, body_lines in jobs.items():
    if job_name == "runtime-assets":
        continue
    body = "\n".join(body_lines)
    for token in (
        "cargo build --release",
        "scripts/package-runtime-asset.sh",
        "scripts/publish-runtime-asset-s3.sh",
        "scripts/check-runtime-asset-s3-existing.sh",
    ):
        if token in body:
            fail(f"{job_name} must not rebuild, repackage, or publish Runtime release artifacts; found {token!r}")

print("Runtime publish workflow static validation passed.")
PY
