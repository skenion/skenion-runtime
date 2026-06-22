# Skenion Runtime

Rust native runtime for graph compilation, scheduling, rendering, preview,
output, plugin hosting, control, and telemetry.

Runtime internals live in a Cargo workspace until external consumers justify extraction.

## Active Surface

The active runtime surface is a ProjectDocumentV02 loader, planner, session API,
and local preview process manager. Graph v0.1 is not an active authoring or
runtime API; legacy v0.1 commands exist only as migration diagnostics while the
remaining internals are being lifted to v0.2.

It can validate and plan:

- Skenion ProjectDocumentV02 JSON files
- graph v0.2 documents resolved against v0.2 node definition manifests
- v0.2 patch libraries and subpatch expansion
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
cargo run -- validate-project --project path/to/project-v0.2.json
cargo run -- plan --project path/to/project-v0.2.json --format text
cargo run -- plan --project path/to/project-v0.2.json --format json
cargo run -- run --project path/to/project-v0.2.json --frames 2 --format json
```

The preview process manager is exposed through the Runtime session HTTP API.
Standalone preview child commands consume prepared plan or preview-document
artifacts and are not the active graph authoring API.

Legacy v0.1 loader checks are intentionally named as legacy commands:

```sh
cargo run -- legacy-validate-node path/to/node-definition-v0.1.json
cargo run -- legacy-validate-graph path/to/graph-v0.1.json
cargo run -- legacy-validate-project --graph path/to/graph-v0.1.json --nodes path/to/node-definitions-v0.1
cargo run -- legacy-preview --graph path/to/graph-v0.1.json --nodes path/to/node-definitions-v0.1 --frames 300
cargo run -- legacy-audio-plan --graph path/to/graph-v0.1.json --nodes path/to/node-definitions-v0.1 --format json
cargo run -- legacy-audio-output --graph path/to/graph-v0.1.json --nodes path/to/node-definitions-v0.1 --duration-ms 1000
```

## Status

Bootstrap repository for the Skenion project. Implementation follows the public architecture and release rules defined in [EchoVisionLab/skenion](https://github.com/echovisionlab/skenion).

## License And Credit

This repository is licensed under the Apache License, Version 2.0.

Redistributions must preserve copyright, license, and NOTICE information as required by Apache-2.0. If Skenion helps your artwork, research, publication, installation, or tool, please credit Skenion and EchoVisionLab.
