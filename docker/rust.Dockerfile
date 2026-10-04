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
