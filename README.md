# Orca Slicer API

Standalone AGPL-3.0-or-later HTTP sidecar for OrcaSlicer `2.4.2` at commit `8500fcdccaa10b5099ac20d252af3a7c560046f1`.

The container sparse-fetches that exact upstream commit as source data. Rust parses `Tab.cpp`, `PrintConfig.cpp`, `PrintConfig.hpp`, and `PrintConfigConstants.hpp` to generate the process schema. The official OrcaSlicer `2.4.2` AppImage is checksum-pinned and supplies the slicing CLI; the normal image build compiles only Rust.

## API

Service listens on port `3000`.

- `GET /health`
- `GET /source`
- `GET /capabilities`
- `GET /schema/process`
- `GET /schema/printer`
- `GET /schema/filament`
- `GET /profiles/bundled`
- `POST /slice`
- `GET /slice/progress/{request_id}`

`POST /slice` accepts LayerCove multipart fields: `file`, `printerProfile`, `presetProfile`, repeated `filamentProfile`, `plate`, `exportType`, `arrange`, `schemaHash`, `processOverrides`, and `requestId`. Profile parts may contain complete profile JSON or an exact four-field system stub (`type`, `name`, `inherits`, `from`) whose `inherits` name resolves through the image's trusted bundled-profile catalog. Stub profiles materialize their trusted official ancestor chain before slicing; missing, ambiguous, cyclic, unknown, or non-system profiles are rejected without treating request data as a filesystem path. `processOverrides` is an optional JSON object that requires a matching `schemaHash` and `presetProfile`; option keys, global scope, values, ranges, and choices are validated against the authoritative process schema before merging into the materialized process. Overrides cannot change profile identity or provenance fields. `modelState` is unsupported: `model_state` remains false, and advanced per-object preparation is not available.

Each slice gets a private Orca `--pipe` FIFO under `/app/data/jobs`. JSON events update the latest progress snapshot while Orca runs. Processes run in private process groups that are killed on timeout or service/request shutdown.

Capability flags are evidence gates. No synthetic schema, model, progress, metadata, or slice artifact is returned.

## Guided calibration engine

The Rust shim generates geometry and profile settings, then slices them with the existing checksum-pinned Orca AppImage. It never builds Orca from source. The release gate inspects all five real Bambu 3MF and Klipper G-code artifacts without sending a printer command.

`GET /capabilities` reports `calibration.available`, version `1`, and supported steps. Calibration uses the normal `ORCA_CLI_PATH` and trusted bundled profiles. No separate calibration binary or resources directory is required.

Calibration `POST /slice` replaces `file` with a JSON `calibration` field containing `step`, `lowest`, `highest`, `increment`, `baseline`, and `previous_results`. It requires printer/process/one filament profile and the matching `schemaHash`. Steps are `temperature`, `flow_rate`, `pressure_advance`, `retraction`, and `volumetric_flow`. `previous_results` contains preceding confirmed values. Model uploads, `modelState`, `processOverrides`, and other plates cannot be combined with calibration. The step size must reach the highest test value. Temperature requires whole degrees and cannot exceed the filament profile's maximum temperature.

Temperature runs hottest at the bottom, with section height `25 × nozzle diameter` mm. Flow tiles carry sample numbers starting at 1. Pressure advance and volumetric flow use 1 mm bands from the base. Retraction uses 1 mm bands above a 0.4 mm base. Retraction processing changes only recognized paired relative-E moves inside the generated model, preserving startup/shutdown and deposition moves. Unsupported custom extrusion fails closed. Bambu 3MF plate checksums are recomputed after rewriting. Native time/filament metadata remains the estimate from slicing at the maximum retraction distance.

Artifact checks prove file generation and value changes, not physical calibration quality. The user must print, inspect, and choose each result. The sidecar does not infer a result from a photo.

## Schema extraction

Rust reads the pinned Orca source files listed above without initializing GUI runtime, recovers process page, group, and option order, validates registry metadata and scopes, and hashes canonical compact JSON. `Preset.cpp` supplies printer and filament key lists, including motion axes and filament overrides. Vector metadata includes item types, defaults, bounds, choices, units, and nullable values. `tests/pinned_source.rs` checks the pinned source's 342 process, 160 printer, and 126 filament options and deterministic repeated extraction. Each schema response has its own content hash and the same engine/image identity.

## Development

Requires Rust `1.97.1`.

```sh
cargo +1.97.1 fmt --all --check
cargo +1.97.1 clippy --all-targets -- -D warnings
cargo +1.97.1 test
```

```sh
docker build -t orca-slicer-api:local .
```

Runtime requires immutable image identity:

```sh
ORCA_RUNTIME_DIGEST=sha256:<64-lowercase-hex> docker compose up
```

Required service environment:

- `ORCA_IMAGE_DIGEST`: immutable OCI manifest digest
- `ORCA_SCHEMA_PATH`: generated contract path
- `ORCA_PROFILES_PATH`: generated public bundled-profile index
- `ORCA_PROFILE_SOURCE_PATH`: trusted bundled-profile source root used to materialize system stubs

Runtime supports a read-only root filesystem with only `/app/data` writable, UID/GID `10001`, dropped capabilities, and no privilege escalation.

## Release gate

Do not publish until schema extraction is byte-identical across two clean builds, representative slices produce parseable real artifacts and metadata, progress advances before completion, and the restricted non-root/read-only container run passes. `cancel` remains false until an authorized cancellation endpoint is implemented and tested end to end.

## License

Service is GNU Affero General Public License v3 or later. See `LICENSE`, `NOTICE`, and `THIRD_PARTY.md`. OrcaSlicer remains separately copyrighted by SoftFever and contributors under its upstream terms.
