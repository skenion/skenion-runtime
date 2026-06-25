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
ci_path = repo_root / ".github/workflows/ci.yml"
release_downloads_path = repo_root / "scripts/update-runtime-release-downloads.sh"
ci_text = ci_path.read_text(encoding="utf-8")
release_downloads_script = release_downloads_path.read_text(encoding="utf-8")

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


FORBIDDEN_USER_FACING_TRIPLES = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
)
ALL_RUST_TRIPLES = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)


def workflow_run_blocks(workflow_lines):
    for index, line in enumerate(workflow_lines):
        match = re.match(r"^(\s*)run:\s*\|\s*$", line)
        if not match:
            continue
        base_indent = len(match.group(1))
        block = []
        for body_line in workflow_lines[index + 1 :]:
            if body_line.strip() and len(body_line) - len(body_line.lstrip(" ")) <= base_indent:
                break
            block.append(body_line)
        yield index + 1, block


def workflow_steps(job_lines):
    steps = []
    current = None

    for index, line in enumerate(job_lines, start=1):
        if re.match(r"^      - (?:id:|name:|uses:)", line):
            if current is not None:
                steps.append(current)
            current = {"start": index, "lines": [line]}
        elif current is not None:
            current["lines"].append(line)

    if current is not None:
        steps.append(current)

    return steps


def step_text(step):
    return "\n".join(step["lines"])


def steps_containing(steps, token):
    return [step for step in steps if token in step_text(step)]


def require_single_step(steps, token, label):
    matches = steps_containing(steps, token)
    if len(matches) != 1:
        fail(f"runtime-assets job must contain exactly one {label}; found {len(matches)}")
    return matches[0]


def assert_no_summary_user_facing_target_leaks():
    violations = []
    for start_line, block in workflow_run_blocks(lines):
        if not any("GITHUB_STEP_SUMMARY" in line for line in block):
            continue
        for offset, line in enumerate(block, start=1):
            stripped = line.strip()
            if not stripped.startswith("echo "):
                continue
            if "${TARGET}" in stripped:
                violations.append(
                    f"publish.yml:{start_line + offset}: workflow summary must not echo internal TARGET: {stripped}"
                )
            if "matrix.target" in stripped:
                violations.append(
                    f"publish.yml:{start_line + offset}: workflow summary must not echo matrix.target: {stripped}"
                )
            for triple in ALL_RUST_TRIPLES:
                if triple in stripped:
                    violations.append(
                        f"publish.yml:{start_line + offset}: workflow summary must not echo Rust target triple {triple!r}: {stripped}"
                    )

    if violations:
        fail("publish workflow summary contains user-facing Runtime target leak(s):\n" + "\n".join(violations))


def user_facing_unknown_linux_allowlist(path, line):
    relative = path.relative_to(repo_root).as_posix()
    stripped = line.strip()

    if relative == ".github/workflows/publish.yml":
        return re.fullmatch(r"- target: (?:x86_64|aarch64)-unknown-linux-gnu", stripped)

    if relative == ".github/workflows/ci.yml":
        return (
            stripped == "x86_64-unknown-linux-gnu \\"
            or stripped in {
                'assert manifest["target"] == "x86_64-unknown-linux-gnu"',
                'assert manifest["rustTargetTriple"] == "x86_64-unknown-linux-gnu"',
                "if grep -q 'unknown-linux-gnu' \"${output}\"; then",
                'echo "::error::Release notes must not surface Rust target triples in public Runtime download links."',
            }
        )

    if relative in {
        "scripts/check-runtime-asset-s3-existing.sh",
        "scripts/package-runtime-asset.sh",
        "scripts/publish-runtime-asset-s3.sh",
    }:
        return re.fullmatch(r"(?:x86_64|aarch64)-unknown-linux-gnu\)", stripped)

    if relative == "scripts/validate-runtime-asset-packaging.sh":
        return stripped == 'linux_target="x86_64-unknown-linux-gnu"'

    if relative == "scripts/validate-runtime-asset-s3-publisher.sh":
        return stripped in {
            'target="x86_64-unknown-linux-gnu"',
            "target=x86_64-unknown-linux-gnu",
            'assert manifest["target"] == "x86_64-unknown-linux-gnu"',
            'assert manifest["rustTargetTriple"] == "x86_64-unknown-linux-gnu"',
            'assert "unknown-linux-gnu" not in manifest["artifact"]["publicUrl"]',
        }

    return False


def assert_no_user_facing_unknown_linux_leaks():
    scan_paths = [
        workflow_path,
        ci_path,
        release_downloads_path,
        repo_root / "scripts/check-runtime-asset-s3-existing.sh",
        repo_root / "scripts/package-runtime-asset.sh",
        repo_root / "scripts/publish-runtime-asset-s3.sh",
        repo_root / "scripts/validate-runtime-asset-packaging.sh",
        repo_root / "scripts/validate-runtime-asset-s3-publisher.sh",
    ]
    scan_paths.extend(path for path in repo_root.glob("README*") if path.is_file())
    for directory_name in ("docs", ".github"):
        directory = repo_root / directory_name
        if directory.is_dir():
            scan_paths.extend(path for path in directory.rglob("*") if path.is_file())

    seen = set()
    violations = []
    for path in scan_paths:
        if path in seen:
            continue
        seen.add(path)
        try:
            path_lines = path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            continue
        for line_number, line in enumerate(path_lines, start=1):
            if not any(token in line for token in FORBIDDEN_USER_FACING_TRIPLES):
                continue
            if user_facing_unknown_linux_allowlist(path, line):
                continue
            violations.append(f"{path.relative_to(repo_root)}:{line_number}: {line.strip()}")

    if violations:
        fail(
            "unknown-linux-gnu may appear only in internal target/provenance allowlisted lines; "
            "user-facing Runtime distribution leak(s):\n" + "\n".join(violations)
        )


assert_no_user_facing_unknown_linux_leaks()
assert_no_summary_user_facing_target_leaks()

required_jobs = {"runtime-assets", "release-downloads"}
missing = required_jobs - set(jobs)
if missing:
    fail(f"publish workflow is missing required jobs: {sorted(missing)}")

for forbidden in (
    "actions/upload-artifact@",
    "actions/download-artifact@",
    "actions/upload-release-asset@",
    "softprops/action-gh-release@",
    "gh release upload",
):
    if forbidden in workflow_text:
        fail(f"publish workflow must not use GitHub or Actions assets for Runtime binary handoff; found {forbidden!r}")

for forbidden in (
    "scripts/check-local-contracts-integration.sh",
    "SKENION_CONTRACTS_RUST_PATH",
    "patch.crates-io",
):
    if forbidden in workflow_text:
        fail(f"publish workflow must not use developer-only local Contracts integration; found {forbidden!r}")

runtime_asset_steps = workflow_steps(jobs["runtime-assets"])
runtime_assets = "\n".join(jobs["runtime-assets"])
for line in jobs["runtime-assets"]:
    stripped = line.strip()
    if not stripped.startswith("name:"):
        continue
    if "matrix.target" in stripped:
        fail("runtime-assets job or step names must not expose Rust target triples via matrix.target")
    for forbidden_public_token in ALL_RUST_TRIPLES:
        if forbidden_public_token in stripped:
            fail(f"runtime-assets job or step names must not expose Rust target triple {forbidden_public_token!r}")

if "scripts/check-runtime-asset-s3-existing.sh" not in runtime_assets:
    fail("runtime-assets job must check DSUB S3 before building release binaries")
if "cargo build --release" not in runtime_assets:
    fail("runtime-assets job must build the release binary exactly once per target")
if "scripts/package-runtime-asset.sh" not in runtime_assets:
    fail("runtime-assets job must package the release binary exactly once per target")
if "scripts/publish-runtime-asset-s3.sh" not in runtime_assets:
    fail("runtime-assets job must publish the package produced in the same job attempt")
if "GitHub Release manifest asset" in runtime_assets:
    fail("runtime-assets summary must not advertise metadata-only GitHub Release manifest assets")
if "cache-hit" in runtime_assets:
    fail("runtime-assets job must not use GitHub Actions cache hits as Runtime release artifact truth")

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
for token, label in (
    ("Install Linux native dependencies", "native dependency installation"),
    ("rustup show", "Rust toolchain inspection"),
    ("rustup target list --installed", "Rust target installation check"),
    ("rustup target add", "Rust target installation"),
    ("Swatinem/rust-cache@v2", "Rust release cache"),
    ("cargo build --release", "release cargo build"),
    ("scripts/package-runtime-asset.sh", "runtime asset packaging"),
    ("--use-existing-manifest", "Runtime artifact publish"),
):
    if existing_index > runtime_assets.index(token):
        fail(f"runtime-assets job must check S3 before {label}")

for token, label in (
    ("Install Linux native dependencies", "native dependency installation"),
    ("rustup show", "Rust toolchain inspection"),
    ("rustup target list --installed", "Rust target installation check"),
    ("rustup target add", "Rust target installation"),
    ("Swatinem/rust-cache@v2", "Rust release cache"),
    ("cargo build --release", "release cargo build command"),
    ("scripts/smoke-runtime-binary.sh", "runtime binary smoke"),
    ("scripts/package-runtime-asset.sh", "runtime asset packaging"),
    ("Generate Runtime artifact manifest", "runtime artifact manifest generation"),
    ("actions/attest@v4", "runtime artifact attestation"),
    ("--use-existing-manifest", "Runtime artifact publish"),
):
    step = require_single_step(runtime_asset_steps, token, label)
    if "if: steps.existing.outputs.exists != 'true'" not in step_text(step):
        fail(f"runtime-assets {label} must be skipped when DSUB S3 already has the immutable artifact")

rust_target_step = require_single_step(runtime_asset_steps, "rustup target add", "conditional Rust target add step")
if "steps.rust-target.outputs.installed != 'true'" not in step_text(rust_target_step):
    fail("runtime-assets Rust target add step must be gated by the installed-target check")
if "rustup target list --installed | grep -Fxq \"${TARGET}\"" not in runtime_assets:
    fail("runtime-assets job must check installed Rust targets before running rustup target add")

aws_probe_step = require_single_step(runtime_asset_steps, "command -v aws", "AWS CLI availability probe")
if "available=true" not in step_text(aws_probe_step) or "available=false" not in step_text(aws_probe_step):
    fail("runtime-assets AWS CLI availability probe must record whether aws is already on PATH")
aws_install_step = require_single_step(runtime_asset_steps, "python -m pip install awscli", "conditional AWS CLI install step")
if "if: steps.aws-cli.outputs.available != 'true'" not in step_text(aws_install_step):
    fail("runtime-assets AWS CLI install must run only when aws is missing")
if "GITHUB_PATH" not in step_text(aws_install_step) or "sysconfig.get_path(\"scripts\")" not in step_text(aws_install_step):
    fail("runtime-assets AWS CLI install must add the Python scripts directory to GITHUB_PATH")
setup_python_step = require_single_step(runtime_asset_steps, "actions/setup-python@v6", "conditional Python setup step")
if "if: steps.aws-cli.outputs.available != 'true'" not in step_text(setup_python_step):
    fail("runtime-assets Python setup must run only when aws is missing")

cache_step = require_single_step(runtime_asset_steps, "Swatinem/rust-cache@v2", "Rust release cache step")
cache_text = step_text(cache_step)
for token in (
    "prefix-key: skenion-runtime-release-assets",
    "runner.os",
    "runner.arch",
    "matrix.platform_slug",
    "matrix.target",
    "profile-release",
    "key: cargo-lockfile-v1",
    'add-rust-environment-hash-key: "true"',
    'env-vars: "CARGO CC CFLAGS CXX CMAKE RUST TARGET"',
    'cache-targets: "true"',
    'cache-all-crates: "true"',
    'cache-on-failure: "true"',
):
    if token not in cache_text:
        fail(f"runtime-assets Rust cache step must make cache criteria explicit; missing {token!r}")

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

for forbidden_public_token in ALL_RUST_TRIPLES:
    if forbidden_public_token in release_downloads_script:
        fail(f"release download generator must not expose Rust target triple {forbidden_public_token!r}")

for public_fixture_text in (workflow_text, ci_text, release_downloads_script):
    if re.search(r"skenion-runtime/v[^\n]*unknown-linux-gnu", public_fixture_text):
        fail("public Runtime release links must not contain unknown-linux-gnu target triples")
    if re.search(r"skenion-runtime-v[^\n]*unknown-linux-gnu", public_fixture_text):
        fail("public Runtime release filenames must not contain unknown-linux-gnu target triples")
    if re.search(r"skenion-runtime-v(?:\d+\.\d+\.\d+|\$\{version\}|\{version\})-[A-Za-z0-9-]+(?:\.tar\.gz|\.zip)", public_fixture_text):
        fail("public Runtime release filenames must be raw binaries, not .tar.gz or .zip archives")
    if re.search(r"windows-(?:x64|arm64)[^\n]*\.tar\.gz", public_fixture_text):
        fail("Windows Runtime public binaries must use .exe, not .tar.gz")

if "skenion-runtime-v{version}-{platform_slug}{extension}" not in release_downloads_script:
    fail("release download generator must include '-v<version>-' in public Runtime binary filenames")

for forbidden in ("gh release upload", "actions/upload-release-asset@", "softprops/action-gh-release@"):
    if forbidden in release_downloads_script:
        fail(f"release download updater must not upload Runtime GitHub Release assets; found {forbidden!r}")

release_downloads = "\n".join(jobs["release-downloads"])
if "scripts/update-runtime-release-downloads.sh" not in release_downloads:
    fail("release-downloads job must update GitHub Release notes with DSUB S3 download links")
if "--delete-github-manifest-assets" not in release_downloads:
    fail("release-downloads job must remove old metadata-only GitHub Release manifest assets")
if "ref: main" not in release_downloads:
    fail("release-downloads job must checkout main so workflow_dispatch can repair older release tags")

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
