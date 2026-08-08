# Orca Slicer API

Standalone AGPL-3.0-or-later HTTP sidecar for OrcaSlicer `2.4.2` at commit `8500fcdccaa10b5099ac20d252af3a7c560046f1`.

The container sparse-fetches that exact upstream commit as source data. Rust parses `Tab.cpp`, `PrintConfig.cpp`, `PrintConfig.hpp`, and `PrintConfigConstants.hpp` to generate the process schema. The official OrcaSlicer `2.4.2` AppImage is checksum-pinned and supplies the slicing CLI; the normal image build compiles only Rust.

## API

Service listens on port `3000`.

- `GET /health`
- `GET /source`
- `GET /capabilities`
- `GET /schema/process`
- `GET /profiles/bundled`
- `POST /slice`
- `GET /slice/progress/{request_id}`

`POST /slice` accepts LayerCove multipart fields: `file`, `printerProfile`, `presetProfile`, repeated `filamentProfile`, `plate`, `exportType`, `arrange`, `schemaHash`, and `requestId`. `modelState` is unsupported: `model_state` remains false, and advanced per-object preparation is not available.

Each slice gets a private Orca `--pipe` FIFO under `/app/data/jobs`. JSON events update the latest progress snapshot while Orca runs. Processes run in private process groups that are killed on timeout or service/request shutdown.

Capability flags are evidence gates. No synthetic schema, model, progress, metadata, or slice artifact is returned.

## Schema extraction

Rust reads the pinned Orca source files listed above without initializing GUI runtime, recovers process page, group, and option order, validates registry metadata and scopes, and hashes canonical compact JSON. `tests/pinned_source.rs` is the extraction oracle: against a fresh sparse checkout it requires 6 pages, 41 groups, 342 complete unique options, and byte-identical repeated extraction.

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

Runtime supports a read-only root filesystem with only `/app/data` writable, UID/GID `10001`, dropped capabilities, and no privilege escalation.

## Release gate

Do not publish until schema extraction is byte-identical across two clean builds, representative slices produce parseable real artifacts and metadata, progress advances before completion, and the restricted non-root/read-only container run passes. `cancel` remains false until an authorized cancellation endpoint is implemented and tested end to end.

## License

Service is GNU Affero General Public License v3 or later. See `LICENSE`, `NOTICE`, and `THIRD_PARTY.md`. OrcaSlicer remains separately copyrighted by SoftFever and contributors under its upstream terms.
