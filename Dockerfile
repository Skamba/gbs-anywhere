# syntax=docker/dockerfile:1
#
#   docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
#     -v gbs-anywhere:/data ghcr.io/skamba/gbs-anywhere
#
# Arguments after the image name are extra `gbs-anywhere` flags
# (see `gbs-anywhere --help`).

FROM rust:1-trixie AS build
# cargo-about writes the third-party notices (about.toml, about.hbs). This is
# its release binary, checked against the SHA-256 GitHub lists for it.
ARG CARGO_ABOUT=0.9.2
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) arch=x86_64 sum=9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af ;; \
      arm64) arch=aarch64 sum=af5169282fb6f84e13471493f405437e43ac517744c9ae12fbe2cdf0a6f0e5a8 ;; \
      *) echo "no cargo-about binary for $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && name="cargo-about-$CARGO_ABOUT-$arch-unknown-linux-musl" \
 && curl -fsSL -o /tmp/about.tar.gz \
      "https://github.com/EmbarkStudios/cargo-about/releases/download/$CARGO_ABOUT/$name.tar.gz" \
 && echo "$sum  /tmp/about.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/about.tar.gz -C /tmp \
 && install "/tmp/$name/cargo-about" /usr/local/cargo/bin/ \
 && rm -r /tmp/about.tar.gz "/tmp/$name"
# Commit shown next to the version (`0.1.0+1a2b3c4`); empty for releases.
ARG GBS_COMMIT=""
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere \
 && mkdir -p /out/data /out/doc \
 && cargo about generate --locked --offline --fail \
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
