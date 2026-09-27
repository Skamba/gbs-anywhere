# syntax=docker/dockerfile:1
#
#   docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
#     ghcr.io/skamba/gbs-anywhere
#
# Arguments after the image name are extra `gbs-anywhere` flags
# (see `gbs-anywhere --help`).

FROM rust:1-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
 && cp target/release/gbs-anywhere /usr/local/bin/gbs-anywhere

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /usr/local/bin/gbs-anywhere /usr/local/bin/gbs-anywhere
# Docker lets unprivileged processes bind low ports inside a container's own
# network namespace, so port 80 works as nonroot with `-p 80:80`.
EXPOSE 80
ENTRYPOINT ["/usr/local/bin/gbs-anywhere", "--no-stdin", "--control", "off"]
