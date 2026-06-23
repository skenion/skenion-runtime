# skenion runtime

Rust native runtime for graph compilation, scheduling, rendering, preview,
output, plugin hosting, control, and telemetry.

Runtime internals live in a Cargo workspace until external consumers justify extraction.

## Active Surface

The active runtime surface is a current 0.1 project loader, planner, session API,
and local preview process manager. Unsupported graph/project/node contract
versions are rejected with structured diagnostics.

It can validate and plan:

- skenion current 0.1 project JSON files
- graph 0.1 documents resolved against node definition manifests
- current 0.1 patch libraries and subpatch expansion
- duplicate node and port ids
- edge endpoint existence
- output-to-input edge direction
- `type + rate` compatibility
- fan-in/fan-out policy checks
- node kind/kindVersion resolution
- graph port snapshots against authoritative node definitions
- topological execution plan skeletons
- cycle detection
- deterministic dummy execution reports
- a local session-driven preview process manager

```sh
cargo run -- validate-project --project path/to/project-0.1.json
cargo run -- plan --project path/to/project-0.1.json --format text
cargo run -- plan --project path/to/project-0.1.json --format json
cargo run -- run --project path/to/project-0.1.json --frames 2 --format json
```

The preview process manager is exposed through the Runtime session HTTP API.
Standalone preview child commands consume prepared plan or preview-document
artifacts and are not graph import or authoring APIs.

## Status

Bootstrap repository for the skenion project. Implementation follows the public architecture and release rules defined in [skenion/skenion](https://github.com/skenion/skenion).

## License And Credit

This repository is licensed under the Apache License, Version 2.0.

Redistributions must preserve copyright, license, and NOTICE information as required by Apache-2.0. If skenion helps your artwork, research, publication, installation, or tool, please credit skenion and the skenion contributors.
