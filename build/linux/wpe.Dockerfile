# WPE WebKit 2.54 with only the WPEPlatform API (no libwpe backends, no DRM/Wayland
# platforms) for the HTML preview spike (`recur-content`'s `html_wpe`). Debian 13 ships
# 2.48 (libwpe API only), so it is built from source into the prefix the .deb would use:
# WebKit compiles in the path of its helper processes.
#
#   docker build --platform linux/$ARCH --build-arg ARCH=$ARCH -f wpe.Dockerfile \
#     -t recur-linux-wpe:$ARCH build/linux
#
# The second stage is the regular build image (build/linux/Dockerfile) plus the prefix.
ARG ARCH=arm64
FROM debian:trixie AS wpe

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl xz-utils build-essential cmake ninja-build pkg-config \
    gperf ruby unifdef python3 perl \
    libglib2.0-dev libharfbuzz-dev libicu-dev libjpeg-dev libepoxy-dev libgcrypt20-dev \
    libsoup-3.0-dev libtasn1-6-dev libxkbcommon-dev libxml2-dev libxslt1-dev libpng-dev \
    libsqlite3-dev libwebp-dev zlib1g-dev libfreetype-dev libfontconfig-dev libwoff-dev \
    libjxl-dev libavif-dev liblcms2-dev libdrm-dev libgbm-dev libseccomp-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev \
    bubblewrap xdg-dbus-proxy \
    && rm -rf /var/lib/apt/lists/*

ARG WPE_VERSION=2.54.2
ARG PREFIX=/opt/recur/wpe
RUN curl -fsSL https://wpewebkit.org/releases/wpewebkit-$WPE_VERSION.tar.xz | tar xJ -C /tmp
# One ninja job per ~1.5 GB of memory: WebKit's unified sources each take that much.
RUN cd /tmp/wpewebkit-$WPE_VERSION && start=$(date +%s) \
    && cmake -S . -B build -G Ninja -DPORT=WPE -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX=$PREFIX -DCMAKE_INSTALL_LIBDIR=lib \
      -DENABLE_WPE_LEGACY_API=OFF -DENABLE_WPE_PLATFORM=ON \
      -DENABLE_WPE_PLATFORM_HEADLESS=ON -DENABLE_WPE_PLATFORM_DRM=OFF -DENABLE_WPE_PLATFORM_WAYLAND=OFF \
      -DENABLE_DOCUMENTATION=OFF -DENABLE_INTROSPECTION=OFF -DENABLE_JOURNALD_LOG=OFF \
      -DENABLE_WEBDRIVER=OFF -DENABLE_SPEECH_SYNTHESIS=OFF -DENABLE_SPELLCHECK=OFF \
      -DENABLE_GAMEPAD=OFF -DUSE_ATK=OFF -DUSE_LIBHYPHEN=OFF -DUSE_LIBBACKTRACE=OFF \
      -DUSE_SYSPROF_CAPTURE=OFF -DENABLE_BUBBLEWRAP_SANDBOX=ON \
    && ninja -C build -j $(( $(awk '/MemTotal/ {print $2}' /proc/meminfo) / 1500000 )) \
    && ninja -C build install \
    && echo "wpe build: $(( $(date +%s) - start ))s on $(nproc) cores" | tee $PREFIX/BUILD_TIME \
    && rm -rf /tmp/wpewebkit-$WPE_VERSION

FROM recur-linux-build:$ARCH
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \
    libsoup-3.0-dev libjxl-dev libavif-dev libwoff-dev liblcms2-dev libgbm-dev libseccomp-dev \
    libgstreamer-plugins-bad1.0-dev bubblewrap xdg-dbus-proxy \
    && rm -rf /var/lib/apt/lists/*
COPY --from=wpe /opt/recur/wpe /opt/recur/wpe
ENV PKG_CONFIG_PATH=/opt/recur/wpe/lib/pkgconfig LD_LIBRARY_PATH=/opt/recur/wpe/lib
USER dev
