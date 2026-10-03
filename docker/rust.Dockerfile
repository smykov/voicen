# Toolchain image for the `core` area (local runs and CI use the same image).
# Base: official Rust image, https://hub.docker.com/_/rust
FROM rust:1.99-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libclang-dev \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add rustfmt clippy
# License check (T-027, decisions #9 #24 D1): cargo-about, pinned. Since 0.9.0 the binary
# needs the `cli` feature (its README: `cargo install --locked --features cli cargo-about`);
# without it `cargo install` succeeds and installs nothing, hence the version check. The
# registry it was built from is dropped; tw-run mounts its own cache over it.
RUN cargo install cargo-about --locked --features cli --version 0.9.2 \
 && cargo about --version \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git \
 && chmod -R a+rwX /usr/local/cargo
ENV CARGO_HOME=/usr/local/cargo
