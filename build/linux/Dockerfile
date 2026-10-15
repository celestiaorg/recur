# Linux build/test image for non-Linux hosts (wails3 task linux:build on a Mac).
# Debian 13: GTK >= 4.14 + webkitgtk-6.0, the Wails v3 default Linux backend.
FROM golang:1.26-trixie

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config gcc libc6-dev git \
    libgtk-4-dev libwebkitgtk-6.0-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
