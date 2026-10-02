# Toolchain image for the `core` area (local runs and CI use the same image).
# Base: official Rust image, https://hub.docker.com/_/rust
FROM rust:1.99-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libclang-dev \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add rustfmt clippy \
 && chmod -R a+rwX /usr/local/cargo
ENV CARGO_HOME=/usr/local/cargo
