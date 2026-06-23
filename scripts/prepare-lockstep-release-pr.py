#!/usr/bin/env python3
"""Prepare Runtime lockstep release PR files.

Release Please can skip a forced lockstep train when no user-facing commits are
present. This script prepares the same release files for the Runtime train
fallback without inventing a conventional feature or fix commit.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import sys


SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
CHANGELOG_HEADING = re.compile(r"^## \[(?P<version>[^\]]+)\]\(")


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def parse_version(version: str) -> tuple[int, int, int]:
    match = SEMVER.fullmatch(version)
    if match is None:
        fail(f"version must be SemVer-like x.y.z; got '{version}'")
    return tuple(int(part) for part in version.split("."))


def read_text(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def write_text(path: pathlib.Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")


def update_manifest(path: pathlib.Path, version: str) -> str:
    manifest = json.loads(read_text(path))
    previous = str(manifest.get(".", "")).strip()
    if not previous:
        fail(".release-please-manifest.json must contain a root package version")

    if parse_version(version) < parse_version(previous):
        fail(f"target version {version} is older than manifest version {previous}")

    manifest["."] = version
    write_text(path, json.dumps(manifest, indent=2) + "\n")
    return previous


def update_cargo_toml(path: pathlib.Path, version: str) -> None:
    text = read_text(path)
    lines = text.splitlines(keepends=True)
    section = ""
    updated_package = False
    updated_contracts = False
    next_lines: list[str] = []

    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped

        if section == "[package]" and re.match(r'^\s*version\s*=', line):
            next_lines.append(re.sub(r'"[^"]+"', f'"{version}"', line, count=1))
            updated_package = True
            continue

        if section == "[dependencies]" and re.match(r'^\s*skenion-contracts\s*=', line):
            if "version" not in line:
                fail("skenion-contracts dependency must declare a version")
            next_lines.append(
                re.sub(r'version\s*=\s*"[^"]+"', f'version = "{version}"', line, count=1)
            )
            updated_contracts = True
            continue

        next_lines.append(line)

    if not updated_package:
        fail("Cargo.toml package version was not found")
    if not updated_contracts:
        fail("Cargo.toml skenion-contracts dependency was not found")

    write_text(path, "".join(next_lines))


def lock_package_has_source_metadata(block: str) -> bool:
    return bool(re.search(r"^(source|checksum) = ", block, flags=re.MULTILINE))


def update_lock_package(block: str, package_name: str, version: str) -> tuple[str, bool]:
    if not re.search(rf'^name = "{re.escape(package_name)}"$', block, flags=re.MULTILINE):
        return block, False
    if lock_package_has_source_metadata(block):
        fail(
            f"Cargo.lock package '{package_name}' has source/checksum metadata; "
            "update it with Cargo instead of the lockstep fallback helper"
        )
    updated, count = re.subn(
        r'^version = "[^"]+"$',
        f'version = "{version}"',
        block,
        count=1,
        flags=re.MULTILINE,
    )
    if count != 1:
        fail(f"Cargo.lock package '{package_name}' must contain exactly one version line")
    return updated, True


def update_cargo_lock(path: pathlib.Path, version: str) -> None:
    text = read_text(path)
    parts = re.split(r"(?=^\[\[package\]\]$)", text, flags=re.MULTILINE)
    found = {"skenion-contracts": 0, "skenion-runtime": 0}
    updated_parts: list[str] = []

    for part in parts:
        next_part = part
        for package_name in found:
            next_part, changed = update_lock_package(next_part, package_name, version)
            if changed:
                found[package_name] += 1
        updated_parts.append(next_part)

    missing = [package_name for package_name, count in found.items() if count == 0]
    if missing:
        fail(f"Cargo.lock package entries were not found: {', '.join(missing)}")
    duplicated = [package_name for package_name, count in found.items() if count > 1]
    if duplicated:
        fail(f"Cargo.lock package entries must be unique: {', '.join(duplicated)}")

    write_text(path, "".join(updated_parts))


def latest_changelog_version(changelog: str, exclude: str) -> str | None:
    for line in changelog.splitlines():
        match = CHANGELOG_HEADING.match(line)
        if match and match.group("version") != exclude:
            return match.group("version")
    return None


def update_changelog(
    path: pathlib.Path,
    version: str,
    previous: str,
    release_date: str,
    repo_url: str,
) -> None:
    changelog = read_text(path)
    if re.search(rf"^## \[{re.escape(version)}\]\(", changelog, flags=re.MULTILINE):
        return

    if previous == version:
        detected_previous = latest_changelog_version(changelog, exclude=version)
        if detected_previous is None:
            fail("could not detect previous changelog version")
        previous = detected_previous

    compare_url = f"{repo_url}/compare/skenion-runtime-v{previous}...skenion-runtime-v{version}"
    entry = (
        f"## [{version}]({compare_url}) ({release_date})\n"
        "\n"
        "\n"
        "### Miscellaneous Chores\n"
        "\n"
        f"* release lockstep Runtime artifacts for Skenion train {version}\n"
        "\n"
    )

    if not changelog.startswith("# Changelog\n\n"):
        fail("CHANGELOG.md must start with '# Changelog' followed by a blank line")

    write_text(path, changelog.replace("# Changelog\n\n", f"# Changelog\n\n{entry}", 1))


def validate_release_files(root: pathlib.Path, version: str) -> None:
    package_version, contracts_version = read_cargo_toml_versions(root / "Cargo.toml")
    if package_version != version:
        fail(f"Cargo.toml package version is {package_version}, expected {version}")
    if contracts_version != version:
        fail(f"skenion-contracts dependency is {contracts_version}, expected {version}")

    locked = read_lock_versions(root / "Cargo.lock")
    for package_name in ("skenion-contracts", "skenion-runtime"):
        locked_version = locked.get(package_name)
        if locked_version != version:
            fail(f"Cargo.lock {package_name} is {locked_version}, expected {version}")


def read_cargo_toml_versions(path: pathlib.Path) -> tuple[str, str]:
    section = ""
    package_version = ""
    contracts_version = ""

    for line in read_text(path).splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped
            continue

        if section == "[package]":
            match = re.match(r'^\s*version\s*=\s*"([^"]+)"', line)
            if match:
                package_version = match.group(1)
        elif section == "[dependencies]" and re.match(r'^\s*skenion-contracts\s*=', line):
            match = re.search(r'version\s*=\s*"([^"]+)"', line)
            if match:
                contracts_version = match.group(1)

    if not package_version:
        fail("Cargo.toml package version was not found")
    if not contracts_version:
        fail("Cargo.toml skenion-contracts dependency version was not found")

    return package_version, contracts_version


def read_lock_versions(path: pathlib.Path) -> dict[str, str]:
    versions: dict[str, str] = {}
    for block in re.split(r"(?=^\[\[package\]\]$)", read_text(path), flags=re.MULTILINE):
        name_match = re.search(r'^name = "([^"]+)"$', block, flags=re.MULTILINE)
        version_match = re.search(r'^version = "([^"]+)"$', block, flags=re.MULTILINE)
        if not name_match or not version_match:
            continue
        package_name = name_match.group(1)
        if package_name in {"skenion-contracts", "skenion-runtime"}:
            versions[package_name] = version_match.group(1)
    return versions


def default_repo_url() -> str:
    repository = os.environ.get("GITHUB_REPOSITORY", "skenion/skenion-runtime")
    return f"https://github.com/{repository}"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "version",
        help="lockstep Runtime release version, for example 0.44.0",
    )
    parser.add_argument("--root", default=".", help="repository root to update")
    parser.add_argument(
        "--date",
        default=dt.date.today().isoformat(),
        help="release date for CHANGELOG.md",
    )
    parser.add_argument(
        "--repo-url",
        default=default_repo_url(),
        help="repository URL for CHANGELOG.md links",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    version = args.version.strip()
    parse_version(version)

    root = pathlib.Path(args.root).resolve()
    previous = update_manifest(root / ".release-please-manifest.json", version)
    update_cargo_toml(root / "Cargo.toml", version)
    update_cargo_lock(root / "Cargo.lock", version)
    update_changelog(
        root / "CHANGELOG.md",
        version,
        previous,
        args.date,
        args.repo_url.rstrip("/"),
    )
    validate_release_files(root, version)


if __name__ == "__main__":
    main()
