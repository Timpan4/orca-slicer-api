#!/usr/bin/env bash
set -euo pipefail

readonly commit=8500fcdccaa10b5099ac20d252af3a7c560046f1
readonly repository=https://github.com/OrcaSlicer/OrcaSlicer.git
destination=${1:?usage: fetch-orca-source.sh DESTINATION}

if [[ -e "$destination" ]]; then
  echo "destination already exists: $destination" >&2
  exit 1
fi

git init --quiet "$destination"
git -C "$destination" remote add origin "$repository"
git -C "$destination" config core.sparseCheckout true
mkdir -p "$destination/.git/info"
printf '%s\n' \
  '/src/slic3r/GUI/Tab.cpp' \
  '/src/libslic3r/PrintConfig.cpp' \
  '/src/libslic3r/PrintConfig.hpp' \
  '/src/libslic3r/PrintConfigConstants.hpp' \
  '/src/libslic3r/Preset.cpp' \
  '/resources/profiles/' \
  '/LICENSE.txt' \
  >"$destination/.git/info/sparse-checkout"
git -C "$destination" fetch --quiet --depth 1 --filter=blob:none origin "$commit"
git -C "$destination" checkout --quiet --detach FETCH_HEAD
test "$(git -C "$destination" rev-parse HEAD)" = "$commit"
test -f "$destination/src/slic3r/GUI/Tab.cpp"
test -f "$destination/src/libslic3r/PrintConfig.cpp"
test -f "$destination/src/libslic3r/PrintConfig.hpp"
test -f "$destination/src/libslic3r/PrintConfigConstants.hpp"
test -d "$destination/resources/profiles"
test -f "$destination/LICENSE.txt"
