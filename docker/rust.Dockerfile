# Toolchain image for the `core` area (local runs and CI use the same image). It also
# type-checks the Windows shell (src-tauri) for x86_64-pc-windows-gnu: `make check-shell-windows`.
# Base: official Rust image, https://hub.docker.com/_/rust
FROM rust:1.99-bookworm
# gcc-mingw-w64-x86-64-win32 (x86_64-w64-mingw32-gcc; its binutils dependency brings -windres)
# and the x86_64-pc-windows-gnu std: make check-shell-windows type-checks src-tauri for Windows
# on Linux (T-056, decisions #63; docs/decisions/ci-toolchain.md). Even under cargo check,
# aws-lc-sys's build script compiles aws-lc with that gcc, and tauri-build compiles the app
# resource with windres. Nothing is linked. The target is baked in, not added at run time:
# tw-run's containers are removed after each command, so a run-time add would download it on
# every run. Its files are world-readable for tw-run's caller uid.
# cmake (unpinned like clang, libclang-dev and mingw gcc; the base image tag fixes the Debian
# suite): it builds the bundled libopus of the opus crate (voicen-core's call codec, T-116),
# natively in check-core and with mingw gcc in check-shell-windows. aws-lc-sys uses its cc
# builder (cmake is only its fallback builder), and whisper-rs-sys skips its cmake build on the
# Linux gate (DOCS_RS=1) (T-115; docs/decisions/ci-toolchain.md).
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libclang-dev gcc-mingw-w64-x86-64-win32 \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add rustfmt clippy \
 && rustup target add x86_64-pc-windows-gnu
# License check (T-027, decisions #9 #24 D1): cargo-about, pinned. Since 0.9.0 the binary
# needs the `cli` feature (its README: `cargo install --locked --features cli cargo-about`);
# without it `cargo install` succeeds and installs nothing, hence the version check. The
# registry it was built from is dropped; tw-run mounts its own cache over it.
RUN cargo install cargo-about --locked --features cli --version 0.9.2 \
 && cargo about --version \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git \
 && chmod -R a+rwX /usr/local/cargo
ENV CARGO_HOME=/usr/local/cargo
# Shared compiler cache (T-217, decisions #304; docs/decisions/ci-toolchain.md "One shared
# sccache cache for every tree's core runs"): sccache, pinned, from the official static musl
# release tarball (a build tool of a never-published image, not a shipped dependency); the
# sha256 is the release's own .sha256 asset, checked again on every build. The entry script is
# the only place RUSTC_WRAPPER is set: when scripts/tw-run mounts the shared host dir at
# /tw-sccache it keeps sccache's cache in the subdirectory SCCACHE_DIR=/tw-sccache/cache and the
# single-writer lock at /tw-sccache/.rw.lock, outside it (sccache's LRU eviction deletes every
# file under SCCACHE_DIR); readers run READ_ONLY. Otherwise, or when the server does not start
# within TW_SCCACHE_START_TIMEOUT (coreutils timeout), it runs plain rustc after one stderr
# line, so CI (which never has the dir) builds exactly as before. SCCACHE_CACHE_SIZE caps the
# cache.
ARG SCCACHE_VERSION=v0.19.0
ARG SCCACHE_SHA256=aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd
RUN set -e; t="sccache-${SCCACHE_VERSION}-x86_64-unknown-linux-musl" \
 && cd /tmp \
 && curl -fsSLo "$t.tar.gz" "https://github.com/mozilla/sccache/releases/download/${SCCACHE_VERSION}/$t.tar.gz" \
 && echo "${SCCACHE_SHA256}  $t.tar.gz" | sha256sum -c - \
 && tar -xzf "$t.tar.gz" "$t/sccache" \
 && install -m 0755 "$t/sccache" /usr/local/bin/sccache \
 && rm -rf "$t" "$t.tar.gz" \
 && sccache --version \
 && command -v flock && command -v timeout
ENV SCCACHE_DIR=/tw-sccache/cache SCCACHE_CACHE_SIZE=10G
COPY voicen-core-entry.sh /usr/local/bin/voicen-core-entry.sh
RUN chmod 0755 /usr/local/bin/voicen-core-entry.sh
ENTRYPOINT ["/usr/local/bin/voicen-core-entry.sh"]
CMD ["bash"]
