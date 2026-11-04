#!/usr/bin/env bash
# Installs the system packages a Recur build needs, for Debian/Ubuntu (apt),
# Fedora/RHEL (dnf) and Arch (pacman), with sudo or doas unless root.
# Run by build/linux/install.sh. The -dev packages match CI's package-linux job
# (.github/workflows/ci.yml); keep them in step.
set -euo pipefail

if [ "$(id -u)" -eq 0 ]; then
  su=()
else
  s=$(command -v sudo || command -v doas || true)
  [ -n "$s" ] || { echo "Need root for system packages: install sudo or doas, or install them yourself" >&2; exit 1; }
  su=("$s")
fi

family=
if [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  for id in ${ID:-} ${ID_LIKE:-}; do
    case $id in
      debian | ubuntu) family=apt; break ;;
      fedora | rhel | centos | rocky | almalinux) family=dnf; break ;;
      arch | manjaro | endeavouros) family=pacman; break ;;
    esac
  done
fi

# Build: a C/C++ toolchain, cmake (aws-lc-sys), pkg-config and git (build/version.sh), plus the
# -dev packages CI's package-linux job installs. The gstreamer plugins are runtime: the app plays
# video through them (the .deb and .rpm depend on them too).
case $family in
  apt)
    "${su[@]}" apt-get update
    "${su[@]}" apt-get install -y build-essential pkg-config cmake git \
      libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
      libfontconfig-dev libfreetype-dev libvulkan-dev libgtk-4-dev libwebkitgtk-6.0-dev \
      libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
      gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
    ;;
  dnf)
    "${su[@]}" dnf install -y gcc gcc-c++ make cmake pkgconf-pkg-config git \
      libxkbcommon-devel libxkbcommon-x11-devel wayland-devel libX11-devel libxcb-devel \
      fontconfig-devel freetype-devel vulkan-loader-devel gtk4-devel webkitgtk6.0-devel \
      gstreamer1-devel gstreamer1-plugins-base-devel \
      gstreamer1-plugins-good gstreamer1-plugins-bad-free
    # libav is in RPM Fusion on Fedora; most video still plays without it.
    "${su[@]}" dnf install -y gstreamer1-plugin-libav ||
      echo "note: gstreamer1-plugin-libav is unavailable (RPM Fusion); some video codecs will not play" >&2
    ;;
  pacman)
    "${su[@]}" pacman -S --needed --noconfirm base-devel cmake git \
      libxkbcommon libxkbcommon-x11 wayland libx11 libxcb fontconfig freetype2 \
      vulkan-icd-loader vulkan-headers gtk4 webkitgtk-6.0 gstreamer gst-plugins-base \
      gst-plugins-good gst-plugins-bad gst-libav
    ;;
  *)
    echo "Unrecognized distro. Install the GTK 4, WebKitGTK 6.0, GStreamer, xkbcommon, Wayland," >&2
    echo "xcb, fontconfig and Vulkan development packages, a C/C++ toolchain," >&2
    echo "cmake and git, then rerun with --no-deps." >&2
    exit 1
    ;;
esac
