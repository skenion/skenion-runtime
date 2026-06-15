# Skenion Runtime

Rust native runtime for graph compilation, scheduling, rendering, preview,
output, plugin hosting, control, and telemetry.

Runtime internals live in a Cargo workspace until external consumers justify extraction.

## Initial Surface

The first runtime surface is a contract loader, not the renderer.

It can validate and plan:

- Skenion Node Definition Manifest v0.1 JSON files
- Skenion Graph Document v0.1 JSON files
- graph documents resolved against a node definition registry
- duplicate node and port ids
- edge endpoint existence
- output-to-input edge direction
- `flow + dataKind` compatibility
- input-only `activation`
- unsupported node permissions
- node kind/kindVersion resolution
- graph port snapshots against authoritative node definitions
- topological execution plan skeletons
- cycle detection
- deterministic dummy execution reports
- a local winit placeholder preview window

```sh
cargo run -- validate-node path/to/node-definition.json
cargo run -- validate-graph path/to/graph.json
cargo run -- validate-project --graph path/to/graph.json --nodes path/to/node-definitions
cargo run -- plan --graph path/to/graph.json --nodes path/to/node-definitions --format text
cargo run -- plan --graph path/to/graph.json --nodes path/to/node-definitions --format json
cargo run -- run --graph path/to/graph.json --nodes path/to/node-definitions --frames 2 --format json
cargo run -- preview --graph path/to/graph.json --nodes path/to/node-definitions --frames 300
```

The preview window is a visual shell only. It advances a placeholder frame
counter from the execution plan and does not perform GPU rendering, video/audio
processing, or script execution yet.

## Status

Bootstrap repository for the Skenion project. Implementation follows the public architecture and release rules defined in [EchoVisionLab/skenion](https://github.com/echovisionlab/skenion).

## License And Credit

This repository is licensed under the Apache License, Version 2.0.

Redistributions must preserve copyright, license, and NOTICE information as required by Apache-2.0. If Skenion helps your artwork, research, publication, installation, or tool, please credit Skenion and EchoVisionLab.
