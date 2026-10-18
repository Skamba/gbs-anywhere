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
# Cargo features, e.g. `bluetooth` for Bluetooth scales. That build needs
# the host's D-Bus: add `-v /run/dbus:/run/dbus:ro` to `docker run`.
ARG FEATURES=""
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked --features "$FEATURES" \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere \
 && mkdir -p /out/data /out/doc \
 && case "$FEATURES" in *bluetooth*) \
      v=$(grep -A1 '^name = "libdbus-sys"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p') \
   && cp "$(ls -d /usr/local/cargo/registry/src/*/libdbus-sys-$v | head -n 1)/vendor/dbus/COPYING" \
        /out/doc/libdbus-COPYING \
   && printf '%s\n' \
        "The Bluetooth build has libdbus built in, from the libdbus-sys $v crate:" \
        "https://crates.io/crates/libdbus-sys/$v (its source is under vendor/dbus)." \
        "libdbus is offered under the Academic Free License 2.1 or the GNU GPL" \
        "version 2 or later; gbs-anywhere uses it under the GNU GPL version 2 or" \
        "later (see libdbus-COPYING). gbs-anywhere's own source:" \
        "https://github.com/Skamba/gbs-anywhere" > /out/doc/libdbus-SOURCE ;; \
    esac

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /usr/local/bin/gbs-anywhere /usr/local/bin/gbs-anywhere
COPY LICENSE /usr/share/doc/gbs-anywhere/LICENSE
# The Bluetooth build's libdbus license and where its source is; empty
# otherwise.
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
