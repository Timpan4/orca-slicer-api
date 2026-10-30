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

The Rust shim loads Orca's own calibration resources, cuts and caps their meshes, applies the GUI's settings, then slices them with the checksum-pinned Orca AppImage. It never builds Orca from source. The CLI decodes the bundled Draco towers to STL. Flow patterns retain their named mesh objects from the bundled 3MF files. The release gate inspects real Bambu 3MF and Klipper G-code artifacts without sending a printer command.

`GET /capabilities` reports `calibration.available`, version `3`, and supported steps. Upgrade LayerCove together with sidecar 0.2.0. Earlier LayerCove contracts are incompatible. Calibration uses `ORCA_CLI_PATH`, trusted bundled profiles, and the image's `/app/orca/resources/calib` resources. Development runs can override the resource root with `ORCA_CALIBRATION_PATH`.

Calibration `POST /slice` replaces `file` with a JSON `calibration` field. Every request contains `step`, `baseline`, and `previous_results`. It requires printer/process/one filament profile and the matching `schemaHash`. Steps are `temperature`, `flow_rate`, `pressure_advance`, `retraction`, and `volumetric_flow`. Non-flow steps require `lowest`, `highest`, and `increment`, and reject `flow_pattern`. The step size must reach the highest test value. Temperature requires increment 5 and bounds that are multiples of 5 within 5..350 °C, the labels covered by the 700 mm bundled tower. The baseline and preceding temperature safety limit remains 500 °C. The filament profile's recommended temperature range does not limit the test.

Flow rate requires `flow_pattern` and rejects all three range fields. Patterns are `yolo`, `yolo_perfectionist`, `pass1`, and `pass2`. The linear patterns use the named modifier as `(baseline + modifier) / baseline`; passes use `1 + modifier / 100`. Each named object keeps Orca's model geometry, scaling, and object settings. `previous_results` contains preceding confirmed values. Model uploads, `modelState`, `processOverrides`, and other plates cannot be combined with calibration. Percentage `initial_layer_line_width` and `line_width` values are converted to millimetres of the nozzle diameter before arrange, avoiding the CLI's raw-number interpretation of percentages.

Temperature runs hottest at the bottom with 10 mm blocks. Tower cut heights match Plater: temperature `10 * ((highest - lowest) / 5 + 1)`, PA `ceil((highest - lowest) / increment) + 1`, retraction `1.4 + (highest - lowest) / increment`, and volumetric `(highest - lowest + 1) / increment` mm. Printable-height checks remain. PA uses `lowest + floor(Z) * increment`; retraction uses `lowest + floor(max(0, Z - 0.4)) * increment`; volumetric flow uses `lowest + Z * increment`, converted to rounded wall speed through Orca's bead area and filament flow. Temperature uses GCode.cpp's stepped layer interpolation.

These upstream height formulas can produce a final-layer value above `highest`. PA's safety maximum of 2 also applies to that value. Requests taller than the native tower mesh are rejected rather than extending its geometry. The mesh cut height is exact; the final printed Z follows Orca's layer grid. For example, the 2.5 mm volumetric model prints its last layer at 2.44 mm with a 0.2 mm first layer and 0.32 mm later layers. Flow patterns retain their original relative layout, centered on the bed, to avoid CLI arrangement path conflicts.

Retraction processing changes only recognized paired relative-E moves, preserving startup/shutdown and deposition moves. Unsupported custom extrusion fails closed. Volumetric processing changes deposited outer-wall feedrates. Bambu 3MF plate checksums are recomputed after rewriting. Native time/filament metadata remains the estimate from the original slice, before postprocessing.

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
