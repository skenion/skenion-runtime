# Codex Agent Context

This repository is one part of the Skenion workspace. Do not treat local code
momentum as the source of truth: before committing, pushing, opening a PR, or
writing PR close keywords, check the relevant GitHub milestone and issue with
`/opt/homebrew/bin/gh`.

Rust tooling is installed through rustup in this environment. Use
`~/.cargo/bin/cargo`, `~/.cargo/bin/rustfmt`, and `~/.cargo/bin/rustup` if the
shell PATH does not include Cargo.

## Local Runtime Development Server

Runtime feature work and Studio/Runtime live validation should run through
`cargo-watch` instead of manual rebuild/restart loops. If the Runtime is not
already running for the selected local port, start it from this repo with:

```sh
~/.cargo/bin/cargo watch -w src -w Cargo.toml -w Cargo.lock -x 'run -- serve --host 127.0.0.1 --port 3761'
```

If `cargo-watch` is missing, install it with
`~/.cargo/bin/cargo install cargo-watch --locked`. When the Runtime panics or a
session/collaboration lock becomes poisoned, restart the watched process before
continuing validation. Do not treat `/health` alone as sufficient after a panic;
also verify a session or feature-specific endpoint.

## Generated Dependency Metadata

Lockfiles and package manifests are repo-owned dependency metadata. This
includes `Cargo.lock`, generated package version constants, and comparable
dependency outputs.

If a build, test, generator, or package manager in this repo updates those files
for a legitimate reason, include and commit that churn with the Runtime slice.
Do not revert dependency metadata merely because it is generated. If the change
is in another repo or outside the assigned write-set, leave it alone and report
it only if it blocks verification.

## Strict v0 Runtime Policy

Skenion v0 does not support legacy, deprecated, or import-only compatibility
paths. Runtime endpoints, CLI commands, payload decoders, sessions, and
capability responses must model the current product surface only. Unsupported
schema, protocol, graph, project, package, manifest, extension, or ABI versions
must be rejected with structured issues rather than migrated, imported,
shimmed, or kept behind deprecated aliases.

The forward graph/project contract label is `0.1`. Runtime should follow
Contracts as the source of truth for that current contract surface. Do not
preserve older meanings as legacy compatibility, and do not keep parallel graph
contract surfaces. If a version field remains, Runtime should accept only exact
current `0.1` for that surface and reject all others.

Default-session compatibility aliases are removal debt, not v0 product behavior.
New Runtime API work should use explicit sessions and current 0.1 project
payloads.

## Component Releases And Compatibility Boundaries

Release Please owns natural component releases for this repository. The hub and
organization Project are operating ledgers, not compatibility authorities and
not component release conductors. Runtime compatibility is proven at the
Runtime boundary: build against the released Contracts crate version, expose
the Runtime API/protocol metadata Studio needs, and verify the release artifact
workflow in Runtime CI. Do not introduce a separate hub-owned compatibility
matrix verifier or push Runtime/Studio artifact evidence into Contracts.

CI must not hardcode or independently enforce a Contracts version, Contracts
line, or supported Contracts range as a compatibility authority. Default CI
should build and test against the `skenion-contracts` version declared by this
repo's `Cargo.toml`/`Cargo.lock` and reject only invalid dependency sources
such as local/path/Git dependencies in release mode. Runtime may expose exact
built-against Contracts provenance and a supported Contracts range in runtime
metadata, but those values must be derived from source-controlled metadata or
the built binary, not duplicated as workflow-owned constants.

Registry publishing and binary release artifacts must be produced only through
GitHub Actions release workflows and Release Please. Local verification may use
dry-run/build commands, but never publish locally.

All release-state writes must happen inside GitHub Actions as well. Do not
create, edit, delete, promote, demote, or repair GitHub Releases, release
assets, tags, prerelease/draft flags, release notes, Runtime binary evidence,
npm packages, crates, or release promotion ledgers from a local shell. This
includes `gh release edit`, `gh release upload`, `gh release delete`, manual tag
mutation, local registry publish, or ad hoc release metadata patches with a
locally exported token. Local commands may inspect state, run dry-run/build
checks, create normal code PRs, or trigger approved `workflow_dispatch` jobs;
the actual release mutation must run in CI with reviewed workflow code and
auditable logs.

Workflows that need cross-repository or release automation credentials must use
the organization Actions secret `GH_TOKEN`. Do not add `RELEASE_PLEASE_TOKEN`,
`SKENION_RELEASE_TRAIN_TOKEN`, or default Actions-token fallbacks for release,
artifact-verification, or promotion workflows.

Runtime is a product binary in v0. Release work must account for multi-arch
sidecar assets, checksums, and desktop/local-managed consumers. Do not add a
Runtime registry publish surface unless a later milestone explicitly scopes a
stable embeddable library API.

## Manager, Worker, And Review Gate Defaults

Codex should operate as a manager/orchestrator on Skenion work. The manager owns
sequencing, milestone and issue hygiene, PR title/body/close-keyword control,
worker assignment, integration, and final reporting. Except for trivial
documentation, context, issue, or status edits, the manager should not directly
modify code. Implementation work and follow-up fixes should be delegated to
focused worker agents, then integrated by the manager. Workers must receive a
clear ownership scope, usually specific files, modules, or repository slices,
and must be told that other agents may be editing nearby code.

Follow-up work is not an exception: if review, CI, or user feedback requires
non-trivial code changes, the manager must assign that work to a worker and send
the completed slice through a separate review gate again. The manager may run
verification and status commands, but should not directly patch non-trivial
implementation code.

Every completed worker slice needs a separate review gate before it is treated
as done. The gate should be a different expert agent from the worker. A gate
review should prioritize correctness, API cleanliness, responsibility
boundaries, readability, test coverage, CI risk, and milestone acceptance
criteria. If the gate fails, the manager must send concrete fixes back to a
worker, then run the gate again until the slice passes or a real blocker is
recorded in the issue. The manager may only make trivial documentation,
context, issue, or status corrections directly.

Worker and reviewer reports must be brief by default. Routine PASS reports,
progress summaries, and commit-readiness notes should use only: PASS/FAIL,
blocking findings, non-blocking follow-ups that change the next action,
verification summary, and next action. Do not include long code-line tours,
exhaustive source references, or repeated evidence in ordinary reports.
File/line references are required for bugs, FAIL reviews, CI failures, security
or data-loss risks, and explicit audit requests; otherwise keep them minimal.
The goal is fast decision-making, not transcript-sized reports.

Default code quality requirements:

- Write code that is easy to read before it is clever.
- Follow clean-code principles: clear names, small responsibilities, explicit
  data flow, predictable control flow, and low incidental coupling.
- Do not introduce interface-based abstraction lightly. Public APIs, traits,
  generated clients, schemas, and extension points must earn their existence and
  remain small, stable, and understandable.
- Keep responsibility ownership clear. Runtime, Studio, Contracts, SDK,
  Examples, and Docs must not duplicate each other's source-of-truth roles.
- UI/UX work must be reviewed for actual workflow quality, not merely rendered
  components.

Issues and milestones are the operating ledger. When work discovers new debt,
missing scope, or a design risk, record it on the relevant GitHub issue or open
a properly milestoned issue before burying it in local context. Close issues
only when the repository-specific acceptance criteria are genuinely complete.
Use `Refs` for partial or cross-repo work and `Closes` only for finished scope.
