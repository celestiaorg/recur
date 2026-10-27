# Linux build/test image for the Rust port on non-Linux hosts
# (wails3 task linux:build:rust on a Mac). Debian 13 like build/linux/Dockerfile:
# GPUI's X11/Wayland backend plus GTK 4, WebKitGTK 6.0 and GStreamer for recur-content, and the
# tools tests shell out to that CI runners have (node, rg, ps, gofmt, python3, xmllint).
FROM debian:trixie

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential pkg-config cmake clang \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
    libfontconfig-dev libfreetype-dev libvulkan-dev libzstd-dev libssl-dev \
    libgtk-4-dev libwebkitgtk-6.0-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav \
    xvfb xauth fonts-dejavu-core ripgrep git nodejs procps golang-go python3 libxml2-utils \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
# Matches rust/rust-toolchain.toml; bump together.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain 1.95 --component rustfmt,clippy \
    && chmod -R a+w $RUSTUP_HOME $CARGO_HOME

# Docker blocks the user namespaces WebKit's bubblewrap sandbox needs; tests that open
# WebKitGTK run under xvfb-run.
ENV WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
# A Docker VM links every test binary at once (one per core); full debug info
# runs it out of memory.
ENV CARGO_PROFILE_DEV_DEBUG=line-tables-only

# Tests run as a regular user, as on CI: permission-failure tests pass as root.
# The cargo volumes start out owned by it.
RUN useradd -m dev && mkdir -p /target /usr/local/cargo/registry \
    && chown dev /target /usr/local/cargo/registry
USER dev
# Tests commit in temp repos; a container hostname gives git no identity to fall back on.
RUN git config --global user.name dev && git config --global user.email dev@example.com

WORKDIR /app/rust
