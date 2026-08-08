# syntax=docker/dockerfile:1.7
ARG ORCA_COMMIT=8500fcdccaa10b5099ac20d252af3a7c560046f1

FROM ubuntu:24.04 AS orca-source
ARG ORCA_COMMIT
RUN test "$ORCA_COMMIT" = "8500fcdccaa10b5099ac20d252af3a7c560046f1" \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY scripts/fetch-orca-source.sh /usr/local/bin/fetch-orca-source
RUN fetch-orca-source /opt/orca-slicer \
    && test "$(git -C /opt/orca-slicer rev-parse HEAD)" = "$ORCA_COMMIT" \
    && rm -rf /opt/orca-slicer/.git

FROM ubuntu:24.04 AS orca-runtime
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN architecture="${TARGETARCH:-$(dpkg --print-architecture)}" \
    && case "$architecture" in \
         amd64) \
           asset=OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.4.2.AppImage; \
           digest=d12fb8c8eac1aecd2dfb6377acd48f994f8fa439ed5292fa532dd82880f029fd ;; \
         arm64) \
           asset=OrcaSlicer_Linux_AppImage_Ubuntu2404_aarch64_V2.4.2.AppImage; \
           digest=e1a07275a25f176626c55a5df39e91bc4476d8c28ee4a3192ff758e29dd5c3ba ;; \
         *) echo "unsupported architecture: $architecture" >&2; exit 2 ;; \
       esac \
    && curl --fail --location --retry 3 \
       "https://github.com/OrcaSlicer/OrcaSlicer/releases/download/v2.4.2/$asset" \
       --output /tmp/orca.AppImage \
    && printf '%s  %s\n' "$digest" /tmp/orca.AppImage | sha256sum --check --strict \
    && chmod +x /tmp/orca.AppImage \
    && cd /opt \
    && /tmp/orca.AppImage --appimage-extract >/dev/null \
    && test -x /opt/squashfs-root/AppRun \
    && test -x /opt/squashfs-root/bin/orca-slicer \
    && test -x /opt/squashfs-root/libexec/orca-slicer-env \
    && rm /tmp/orca.AppImage

FROM rust:1.97.1-bookworm AS rust-builder
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY src ./src
COPY tests ./tests
COPY --from=orca-source /opt/orca-slicer /tmp/orca-slicer
RUN ORCA_SOURCE_DIR=/tmp/orca-slicer \
    cargo test --locked --release --test pinned_source -- --ignored
RUN cargo build --release --locked \
    && ./target/release/schema-export \
       --print-config-source /tmp/orca-slicer/src/libslic3r/PrintConfig.cpp \
       --print-config-header /tmp/orca-slicer/src/libslic3r/PrintConfig.hpp \
       --constants-source /tmp/orca-slicer/src/libslic3r/PrintConfigConstants.hpp \
       --tab-source /tmp/orca-slicer/src/slic3r/GUI/Tab.cpp \
       --output /tmp/process-schema-first.json \
    && ./target/release/schema-export \
       --print-config-source /tmp/orca-slicer/src/libslic3r/PrintConfig.cpp \
       --print-config-header /tmp/orca-slicer/src/libslic3r/PrintConfig.hpp \
       --constants-source /tmp/orca-slicer/src/libslic3r/PrintConfigConstants.hpp \
       --tab-source /tmp/orca-slicer/src/slic3r/GUI/Tab.cpp \
       --output /tmp/process-schema-second.json \
    && cmp /tmp/process-schema-first.json /tmp/process-schema-second.json \
    && mv /tmp/process-schema-first.json /tmp/process-schema.json \
    && ./target/release/profile-index \
       --profiles /tmp/orca-slicer/resources/profiles \
       --output /tmp/profiles.json

FROM ubuntu:24.04 AS runtime
ENV DEBIAN_FRONTEND=noninteractive \
    HOME=/app/data \
    APPDIR=/app/orca \
    ORCA_CLI_PATH=/app/orca/AppRun \
    ORCA_SCHEMA_PATH=/app/schema/process.json \
    ORCA_PROFILES_PATH=/app/schema/profiles.json \
    ORCA_DATA_DIR=/app/data \
    TMPDIR=/app/data/tmp \
    XDG_CACHE_HOME=/app/data/cache \
    XDG_CONFIG_HOME=/app/data/config
RUN mkdir -p /app/data/tmp \
    && apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libglu1-mesa libice6 libopengl0 libsm6 libwebkit2gtk-4.1-0 tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /app/data app \
    && mkdir -p /app/bin /app/data /app/schema /app/orca \
    && chown -R app:app /app
COPY --from=orca-runtime /opt/squashfs-root/ /app/orca/
COPY --from=orca-source /opt/orca-slicer/LICENSE.txt /app/ORCASLICER-LICENSE.txt
COPY --from=rust-builder /src/target/release/orca-slicer-api /app/bin/orca-slicer-api
COPY --from=rust-builder /tmp/process-schema.json /app/schema/process.json
COPY --from=rust-builder /tmp/profiles.json /app/schema/profiles.json
COPY NOTICE THIRD_PARTY.md LICENSE /app/
RUN /app/orca/AppRun --help 2>&1 | grep -q '^OrcaSlicer-2.4.2:'
USER 10001:10001
WORKDIR /app
EXPOSE 3000
ENTRYPOINT ["/usr/bin/tini", "--", "/app/bin/orca-slicer-api"]
