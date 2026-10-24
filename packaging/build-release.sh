#!/bin/sh
# Builds the release binary as a tarball and the Debian package into dist/,
# for the architecture it runs on. image.yml runs it in the official Rust
# image on Debian 12 (rust:1-bookworm), so that both work with glibc 2.36 or
# newer: Raspberry Pi OS 12 and 13, Debian 12 and 13, Ubuntu 24.04. Locally:
#   docker run --rm -v "$PWD:/src" -w /src rust:1-bookworm sh packaging/build-release.sh
set -eu

CARGO_DEB=3.8.0
version=$(grep -m 1 '^version = ' Cargo.toml | cut -d '"' -f 2)
arch=$(dpkg --print-architecture)

sh packaging/install-cargo-about.sh
cargo install cargo-deb --version "$CARGO_DEB" --locked
# cargo-deb works out the package's dependencies with dpkg-shlibdeps.
command -v dpkg-shlibdeps >/dev/null \
  || { apt-get update && apt-get install -y --no-install-recommends dpkg-dev; }

cargo build --release --locked
cargo about generate --locked --fail -o target/THIRD-PARTY-NOTICES about.hbs
cargo deb --no-build --locked

rm -rf dist
mkdir dist
cp "target/debian/gbs-anywhere_${version}-1_${arch}.deb" dist/
name="gbs-anywhere-$version-linux-$arch"
rm -rf "target/$name"
mkdir "target/$name"
cp target/release/gbs-anywhere LICENSE README.md target/THIRD-PARTY-NOTICES "target/$name/"
tar -czf "dist/$name.tar.gz" -C target "$name"

# The package installs (no systemd here, so the service isn't started) and
# its binary runs.
dpkg -i "dist/gbs-anywhere_${version}-1_${arch}.deb"
gbs-anywhere --version
ls -l dist
