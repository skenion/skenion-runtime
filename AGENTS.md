# Codex Agent Context

This repository is one part of the Skenion workspace. Do not treat local code
momentum as the source of truth: before committing, pushing, opening a PR, or
writing PR close keywords, check the relevant GitHub milestone and issue with
`/opt/homebrew/bin/gh`.

Rust tooling is installed through rustup in this environment. Use
`~/.cargo/bin/cargo`, `~/.cargo/bin/rustfmt`, and `~/.cargo/bin/rustup` if the
shell PATH does not include Cargo.

## Strict v0 Runtime Policy

Skenion v0 does not support legacy, deprecated, or import-only compatibility
paths. Runtime endpoints, CLI commands, payload decoders, sessions, and
capability responses must model the current product surface only. Unsupported
schema, protocol, graph, project, package, manifest, extension, or ABI versions
must be rejected with structured diagnostics rather than migrated, imported,
shimmed, or kept behind deprecated aliases.

The forward graph/project contract label is `0.1`. Runtime should follow
Contracts as the source of truth for that current contract surface. Do not
preserve older meanings as legacy compatibility, and do not keep parallel graph
contract surfaces. If a version field remains, Runtime should accept only exact
current `0.1` for that surface and reject all others.

Default-session compatibility aliases are removal debt, not v0 product behavior.
New Runtime API work should use explicit sessions and current 0.1 project
payloads.

## Lockstep Release Train

Skenion v0 releasable packages and applications use the same product release
train version. If the product train is `0.55`, Runtime crates and binary
artifacts publish as `0.55.0` where registries require patch SemVer, and
Studio/contracts/sdk/docs/examples artifacts must belong to that same train.
The release train manifest should name the product train id, Contracts version,
Runtime crate version, Runtime multi-arch binary artifacts, Studio releases,
Manual version, protocol baselines, capability set, checksums, and release
completion gates.

Registry publishing and binary release artifacts must be produced only through
GitHub Actions release workflows and Release Please. Local verification may use
dry-run/build commands, but never publish locally.

Runtime is both a Rust crate and a product binary. Release work must account for
multi-arch sidecar assets, checksums, and desktop/local-managed consumers, not
only crates.io.

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
