#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="${repo_root}/.github/workflows/publish.yml"

python3 - "${workflow}" <<'PY'
import re
import sys
from pathlib import Path

workflow_path = Path(sys.argv[1])
lines = workflow_path.read_text(encoding="utf-8").splitlines()

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


required_jobs = {"runtime-assets"}
missing = required_jobs - set(jobs)
if missing:
    fail(f"publish workflow is missing required jobs: {sorted(missing)}")

workflow_text = "\n".join(lines)
for forbidden in ("actions/upload-artifact@", "actions/download-artifact@"):
    if forbidden in workflow_text:
        fail(f"publish workflow must not use GitHub Actions artifacts for Runtime release handoff; found {forbidden!r}")

runtime_assets = "\n".join(jobs["runtime-assets"])
if "scripts/check-runtime-asset-s3-existing.sh" not in runtime_assets:
    fail("runtime-assets job must check DSUB S3 before building release binaries")
if "cargo build --release" not in runtime_assets:
    fail("runtime-assets job must build the release binary exactly once per target")
if "scripts/package-runtime-asset.sh" not in runtime_assets:
    fail("runtime-assets job must package the release binary exactly once per target")
if "scripts/publish-runtime-asset-s3.sh" not in runtime_assets:
    fail("runtime-assets job must publish the package produced in the same job attempt")
if "gh release upload" not in runtime_assets:
    fail("runtime-assets job must upload metadata-only manifest assets to GitHub Release")

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
