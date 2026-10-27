#!/bin/sh
# Installs cargo-about, which writes the third-party notices (about.toml,
# about.hbs), into $CARGO_HOME/bin. This is its release binary, checked
# against the SHA-256 GitHub lists for it. Used by the Dockerfile and by the
# release packages (build-release.sh), on Debian with the official Rust image.
set -eu

CARGO_ABOUT=0.9.2
case "$(dpkg --print-architecture)" in
  amd64) arch=x86_64 sum=9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af ;;
  arm64) arch=aarch64 sum=af5169282fb6f84e13471493f405437e43ac517744c9ae12fbe2cdf0a6f0e5a8 ;;
  *) echo "no cargo-about binary for $(dpkg --print-architecture)" >&2; exit 1 ;;
esac
name="cargo-about-$CARGO_ABOUT-$arch-unknown-linux-musl"
curl -fsSL -o /tmp/about.tar.gz \
  "https://github.com/EmbarkStudios/cargo-about/releases/download/$CARGO_ABOUT/$name.tar.gz"
echo "$sum  /tmp/about.tar.gz" | sha256sum -c -
tar -xzf /tmp/about.tar.gz -C /tmp
install "/tmp/$name/cargo-about" "${CARGO_HOME:-$HOME/.cargo}/bin/"
rm -r /tmp/about.tar.gz "/tmp/$name"
