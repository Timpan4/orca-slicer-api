#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
dockerfile=$root/Dockerfile
ORCA_RUNTIME_DIGEST=sha256:$(printf '%064d' 0) "$root/scripts/check-container-contract.sh" "$root/docker-compose.yml"
grep -Fq '    read_only: true' "$root/docker-compose.yml"
grep -Fq '    user: "10001:10001"' "$root/docker-compose.yml"
grep -Fq '      - no-new-privileges:true' "$root/docker-compose.yml"
grep -Fq '      - ALL' "$root/docker-compose.yml"

grep -Fqx 'ARG ORCA_COMMIT=8500fcdccaa10b5099ac20d252af3a7c560046f1' "$dockerfile"
grep -Fq 'COPY scripts/fetch-orca-source.sh /usr/local/bin/fetch-orca-source' "$dockerfile"
grep -Fq 'RUN fetch-orca-source /opt/orca-slicer' "$dockerfile"
if grep -Eq 'build_linux\.sh|cmake' "$dockerfile"; then
  echo 'Orca source build is forbidden' >&2
  exit 1
fi

grep -Fq 'OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage' "$dockerfile"
grep -Fq 'd12fb8c8eac1aecd2dfb6377acd48f994f8fa439ed5292fa532dd82880f029fd' "$dockerfile"
grep -Fq 'OrcaSlicer_Linux_AppImage_Ubuntu2404_aarch64_V2.4.2.AppImage' "$dockerfile"
grep -Fq 'e1a07275a25f176626c55a5df39e91bc4476d8c28ee4a3192ff758e29dd5c3ba' "$dockerfile"

grep -Fq 'cargo test --locked --release --test pinned_source -- --ignored' "$dockerfile"
test "$(grep -nF 'cargo test --locked --release --test pinned_source -- --ignored' "$dockerfile" | cut -d: -f1)" -lt \
  "$(grep -nF 'FROM ubuntu:24.04 AS runtime' "$dockerfile" | cut -d: -f1)"
test "$(grep -Fc './target/release/schema-export' "$dockerfile")" -eq 2
grep -Fq 'cmp /tmp/process-schema-first.json /tmp/process-schema-second.json' "$dockerfile"

if awk '/^FROM ubuntu:24.04 AS runtime$/{runtime=1} runtime' "$dockerfile" | grep -Fq 'ORCA_BRIDGE_PATH'; then
  echo 'ORCA_BRIDGE_PATH is forbidden in runtime image' >&2
  exit 1
fi

grep -qx 'target' "$root/.dockerignore"

ci_workflow=$root/.github/workflows/ci.yml
publish_workflow=$root/.github/workflows/publish-container.yml
grep -Fqx '          platforms: linux/amd64' "$ci_workflow"
grep -Fqx '          load: true' "$ci_workflow"
grep -Fqx '          provenance: false' "$ci_workflow"
grep -Fqx '          tags: orca-slicer-api:ci' "$ci_workflow"
grep -Fqx '      - run: python3 tests/container/test_runtime.py orca-slicer-api:ci' "$ci_workflow"
grep -Fqx '          CONTAINER_ENGINE: docker' "$ci_workflow"
grep -Fqx '    needs: [rust, container-contract]' "$ci_workflow"
ci_source_line=$(grep -nF 'scripts/fetch-orca-source.sh "$orca_source"' "$ci_workflow" | cut -d: -f1)
ci_contract_line=$(grep -nF '      - run: tests/container/test_contract.sh' "$ci_workflow" | cut -d: -f1)
test -n "$ci_source_line"
test -n "$ci_contract_line"
test "$ci_source_line" -lt "$ci_contract_line"
grep -Fqx '  gate:' "$publish_workflow"
grep -Fqx '    permissions:' "$publish_workflow"
grep -Fqx '      contents: read' "$publish_workflow"
grep -Fqx '          persist-credentials: false' "$publish_workflow"
grep -Fqx '    needs: gate' "$publish_workflow"
grep -Fqx '      packages: write' "$publish_workflow"
grep -Fqx '      id-token: write' "$publish_workflow"
grep -Fqx '      attestations: write' "$publish_workflow"
if awk '/^permissions:/{scope=1} /^jobs:/{scope=0} scope' "$publish_workflow" | grep -q 'write'; then
  echo 'write permissions must not be global' >&2
  exit 1
fi
if awk '/^  gate:/{scope=1} /^  publish:/{scope=0} scope' "$publish_workflow" | grep -q 'write'; then
  echo 'gate job must be read-only' >&2
  exit 1
fi
test "$(awk '/^  publish:/{scope=1} scope && /: write$/{count++} END{print count+0}' "$publish_workflow")" -eq 3
grep -Fqx '          platforms: linux/amd64' "$publish_workflow"
grep -Fqx '          load: true' "$publish_workflow"
grep -Fqx '          provenance: false' "$publish_workflow"
grep -Fqx '          tags: orca-slicer-api:release-gate' "$publish_workflow"
grep -Fqx '          python3 tests/container/test_runtime.py orca-slicer-api:release-gate' "$publish_workflow"
grep -Fqx '      - uses: docker/login-action@v3' "$publish_workflow"
grep -Fqx '          push: true' "$publish_workflow"
publish_gate_line=$(grep -nF '  gate:' "$publish_workflow" | cut -d: -f1)
publish_login_line=$(grep -nF \
  '      - uses: docker/login-action@v3' "$publish_workflow" | cut -d: -f1)
publish_push_line=$(grep -nF '          push: true' "$publish_workflow" | cut -d: -f1)
test -n "$publish_gate_line"
test -n "$publish_login_line"
test -n "$publish_push_line"
test "$publish_gate_line" -lt "$publish_login_line"
test "$publish_login_line" -lt "$publish_push_line"
grep -Fq 'NoNewPrivs' "$root/tests/container/test_runtime.py"
grep -Fq 'CapBnd' "$root/tests/container/test_runtime.py"
grep -Fq 'Tmpfs' "$root/tests/container/test_runtime.py"
test "$(grep -Fc '          platforms: linux/amd64' "$publish_workflow")" -eq 2
publish_attest_line=$(grep -nF '      - uses: actions/attest-build-provenance@v2' "$publish_workflow" | cut -d: -f1)
test -n "$publish_attest_line"
test "$publish_push_line" -lt "$publish_attest_line"
grep -Fqx '          provenance: mode=max' "$publish_workflow"
grep -Fqx '          sbom: true' "$publish_workflow"
grep -Fqx '          subject-digest: ${{ steps.build.outputs.digest }}' "$publish_workflow"
grep -Fqx '          push-to-registry: true' "$publish_workflow"
