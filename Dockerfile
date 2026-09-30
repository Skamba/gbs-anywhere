# syntax=docker/dockerfile:1
#
#   docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
#     -v gbs-anywhere:/data ghcr.io/skamba/gbs-anywhere
#
# With a Bluetooth scale (Eureka Precisa) the container needs the host's
# network and D-Bus, and root to talk to BlueZ:
#
#   docker run -d --name gbs-anywhere --restart unless-stopped --net=host \
#     --user 0:0 -v /run/dbus:/run/dbus:ro -v gbs-anywhere:/data \
#     ghcr.io/panterro/gbs-anywhere-bt
#
# Arguments after the image name are extra `gbs-anywhere` flags
# (see `gbs-anywhere --help`).

FROM rust:1-trixie AS build
# Commit shown next to the version (`0.1.0+1a2b3c4`); empty for releases.
ARG GBS_COMMIT=""
WORKDIR /src
# Before `COPY . .` so this layer stays cached when the source changes.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libdbus-1-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere \
 && mkdir -p /out/data

# Debian slim instead of distroless: the Bluetooth stack links libdbus, which
# distroless does not ship (with its own dependencies such as libsystemd).
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libdbus-1-3 \
    libssl3t64 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 65532 nonroot \
 && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin nonroot
COPY --from=build /usr/local/bin/gbs-anywhere /usr/local/bin/gbs-anywhere
COPY LICENSE /usr/share/doc/gbs-anywhere/LICENSE
LABEL org.opencontainers.image.licenses="AGPL-3.0-or-later"
# Integrations added in the app are saved here; mount a volume to keep them when
# the container is recreated.
COPY --from=build --chown=65532:65532 /out/data /data
ENV CONFIG_FILE=/data/gbs-anywhere.json
VOLUME ["/data"]
# Same unprivileged user as the distroless image. Docker lets unprivileged
# processes bind low ports inside a container's own network namespace, so
# port 80 works as nonroot with `-p 80:80`. With `--net=host` (Bluetooth)
# port 80 is the host's: run with `--user 0:0` then.
USER 65532:65532
EXPOSE 80
ENTRYPOINT ["/usr/local/bin/gbs-anywhere", "--no-stdin", "--control", "off"]
