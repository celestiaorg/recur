# Linux build/test image for the Rust port on non-Linux hosts
# (wails3 task linux:build:rust on a Mac). Debian 13 like build/linux/Dockerfile:
# GPUI's X11/Wayland backend plus GTK 4 and WebKitGTK 6.0 for recur-content.
FROM debian:trixie

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential pkg-config cmake clang \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
    libfontconfig-dev libfreetype-dev libvulkan-dev libzstd-dev libssl-dev \
    libgtk-4-dev libwebkitgtk-6.0-dev \
    xvfb xauth fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
# Matches rust/rust-toolchain.toml; bump together.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain 1.95 --component rustfmt,clippy \
    && chmod -R a+w $RUSTUP_HOME $CARGO_HOME

# Docker blocks the user namespaces WebKit's bubblewrap sandbox needs; tests that open
# WebKitGTK run under xvfb-run.
ENV WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1

WORKDIR /app/rust
