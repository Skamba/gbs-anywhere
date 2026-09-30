# syntax=docker/dockerfile:1
#
#   docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
#     -v gbs-anywhere:/data ghcr.io/skamba/gbs-anywhere
#
# Arguments after the image name are extra `gbs-anywhere` flags
# (see `gbs-anywhere --help`).

FROM rust:1-trixie AS build
# Commit shown next to the version (`0.1.0+1a2b3c4`); empty for releases.
ARG GBS_COMMIT=""
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere \
 && mkdir -p /out/data

RUN apt-get update && apt-get install -y \
    pkg-config \
    libdbus-1-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /usr/local/bin/gbs-anywhere /usr/local/bin/gbs-anywhere
COPY LICENSE /usr/share/doc/gbs-anywhere/LICENSE
LABEL org.opencontainers.image.licenses="AGPL-3.0-or-later"
# Integrations added in the app are saved here; mount a volume to keep them when
# the container is recreated.
COPY --from=build --chown=65532:65532 /out/data /data
ENV CONFIG_FILE=/data/gbs-anywhere.json
VOLUME ["/data"]
ENV PKG_CONFIG_PATH=/usr/lib/pkgconfig:/usr/lib/aarch64-linux-gnu/pkgconfig/:usr/lib/aarch64-unknown-linux-gnu/pkgconfig
# Docker lets unprivileged processes bind low ports inside a container's own
# network namespace, so port 80 works as nonroot with `-p 80:80`.
RUN ls /usr/lib/ | tee /home/panterro/Projects/gbs-anywhere-bt/build.txt
EXPOSE 80
ENTRYPOINT ["/usr/local/bin/gbs-anywhere", "--no-stdin", "--control", "off"]