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
WORKDIR /src

# The dependencies first, in a layer of their own built from Cargo.toml and
# Cargo.lock alone, so that a change to the sources rebuilds only
# gbs-anywhere itself. An empty stand-in crate takes its place here, and
# `cargo clean -p` drops what was built of it, so the real sources below
# always compile. Layers, not cache mounts: CI keeps layers between runs
# (image.yml, cache-to), cache mounts it does not. The notices depend only on
# these files too. cargo-about may download crates: it reads every crate in
# Cargo.lock, including Windows-only ones the build never fetched.
COPY Cargo.toml Cargo.lock about.toml about.hbs ./
RUN mkdir src \
 && echo 'fn main() {}' > src/main.rs \
 && touch src/lib.rs \
 && cargo build --release --locked \
 && cargo clean --release --locked -p gbs-anywhere \
 && mkdir -p /out/data /out/doc \
 && cargo about generate --locked --fail \
      -o /out/doc/THIRD-PARTY-NOTICES about.hbs \
 && rm -r src

COPY src/ src/
COPY web/ web/
# Commit shown next to the version (`0.1.0+1a2b3c4`); empty for releases.
# It differs on every commit, so it comes after the dependency layer: only
# src/lib.rs reads it (`option_env!`).
ARG GBS_COMMIT=""
RUN cargo build --release --locked \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere

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
