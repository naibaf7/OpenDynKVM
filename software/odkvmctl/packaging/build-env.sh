#!/bin/sh
# Build environment for the Linux packages (cargo xtask package): Debian 12, whose glibc 2.36 every current Debian and
# Ubuntu has, plus a C linker. xtask runs this in a debian:bookworm container and keeps the result as the image
# opendyn-kvm-build:bookworm. Rust is not installed here: the host's rustup toolchain and cargo cache are mounted
# (rustup's toolchains run on glibc 2.17 and later).
set -e
# Rootless podman without subordinate ids cannot switch apt to its _apt user.
apt="apt-get -o APT::Sandbox::User=root"
$apt update
$apt install -y --no-install-recommends gcc libc6-dev ca-certificates
rm -rf /var/lib/apt/lists/*
