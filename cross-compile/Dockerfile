# Cross-build environment for the pluto73 Rust app on macOS and Windows

FROM --platform=linux/amd64 debian:bookworm-slim

ARG TOOLCHAIN_TARBALL=gcc-linaro-7.5.0-2019.12-x86_64_arm-linux-gnueabihf.tar.xz

# gcc/libc6-dev here are the *host* compiler: cargo needs them for build scripts and proc-macros, which run on the build machine, not on the Pluto.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl xz-utils make git pkg-config gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*

COPY ${TOOLCHAIN_TARBALL} /tmp/toolchain.tar.xz
RUN mkdir -p /opt/toolchain \
    && tar -xf /tmp/toolchain.tar.xz -C /opt/toolchain --strip-components=1 \
    && rm /tmp/toolchain.tar.xz

# libiio and its dependency closure, taken off the device itself (libiio v0.25, matching /usr/lib/libiio.so.0.25 there).
COPY libs/ /opt/toolchain/arm-linux-gnueabihf/libc/usr/lib/
RUN cd /opt/toolchain/arm-linux-gnueabihf/libc/usr/lib \
    && ln -sf libiio.so.0.25 libiio.so.0 \
    && ln -sf libiio.so.0 libiio.so

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/opt/toolchain/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
    && rustup target add armv7-unknown-linux-gnueabihf \
    && chmod -R a+w /usr/local/cargo
