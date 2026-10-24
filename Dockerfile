# syntax=docker/dockerfile:1
#
#   docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
#     -v gbs-anywhere:/data ghcr.io/skamba/gbs-anywhere
#
# Arguments after the image name are extra `gbs-anywhere` flags
# (see `gbs-anywhere --help`).

FROM rust:1-trixie AS build
# cargo-about writes the third-party notices (about.toml, about.hbs).
COPY packaging/install-cargo-about.sh /tmp/
RUN sh /tmp/install-cargo-about.sh
# Commit shown next to the version (`0.1.0+1a2b3c4`); empty for releases.
ARG GBS_COMMIT=""
WORKDIR /src
COPY . .
# cargo-about may download crates: it reads every crate in Cargo.lock,
# including Windows-only ones the build itself never fetched.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere \
 && mkdir -p /out/data /out/doc \
 && cargo about generate --locked --fail \
      -o /out/doc/THIRD-PARTY-NOTICES about.hbs

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /usr/local/bin/gbs-anywhere /usr/local/bin/gbs-anywhere
COPY LICENSE /usr/share/doc/gbs-anywhere/LICENSE
# The licenses of the Rust crates built in.
COPY --from=build /out/doc/ /usr/share/doc/gbs-anywhere/
LABEL org.opencontainers.image.licenses="AGPL-3.0-or-later"
# What is set up in the app (integrations, settings), an E65/E80 pairing and
# the shot history are saved here; mount a volume to keep them when the
# container is recreated.
COPY --from=build --chown=65532:65532 /out/data /data
ENV CONFIG_FILE=/data/gbs-anywhere.json
VOLUME ["/data"]
# Docker lets unprivileged processes bind low ports inside a container's own
# network namespace, so port 80 works as nonroot with `-p 80:80`.
EXPOSE 80
ENTRYPOINT ["/usr/local/bin/gbs-anywhere", "--no-stdin", "--control", "off"]
