# Linux build/test image for non-Linux hosts (mise run linux:build on a Mac).
# Ubuntu 24.04, CI's runner (bump with ci.yml's runs-on): GPUI's X11/Wayland backend plus GTK 4,
# WebKitGTK 6.0 and GStreamer for recur-content, and the tools tests shell out to that CI runners
# have (node, rg, ps, gofmt, python3, xmllint).
FROM node:24.22.2-bookworm-slim AS node
FROM ubuntu:24.04

RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential pkg-config cmake clang \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
    libfontconfig-dev libfreetype-dev libvulkan-dev libzstd-dev libssl-dev \
    libgtk-4-dev libwebkitgtk-6.0-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav \
    xvfb xauth fonts-dejavu-core ripgrep git procps golang-go python3 libxml2-utils \
    && rm -rf /var/lib/apt/lists/*
# Node from the official image at mise.toml's version, as CI gets it; Ubuntu's nodejs is 18.
COPY --from=node /usr/local/bin/node /usr/local/bin/node

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
# Matches rust-toolchain.toml; bump together.
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
# The cargo volumes start out owned by it, uid 1000 (the image's own `ubuntu` user goes).
RUN userdel -r ubuntu && useradd -m dev && mkdir -p /target /usr/local/cargo/registry \
    && chown dev /target /usr/local/cargo/registry
USER dev
# Tests commit in temp repos; a container hostname gives git no identity to fall back on.
RUN git config --global user.name dev && git config --global user.email dev@example.com

WORKDIR /app
