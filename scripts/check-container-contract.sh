#!/usr/bin/env bash
set -euo pipefail

compose_file=${1:-docker-compose.yml}
if ! grep -Eq 'ORCA_RUNTIME_DIGEST:\?set .*immutable' "$compose_file"; then
  echo 'compose must require immutable ORCA_RUNTIME_DIGEST' >&2
  exit 1
fi
if ! grep -q 'internal: true' "$compose_file"; then
  echo 'compose must use an internal network' >&2
  exit 1
fi
