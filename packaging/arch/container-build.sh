#!/usr/bin/env bash
# Builds the Arch package as root inside an archlinux container (CI and release
# workflows). git must already be installed and the repository checked out with it.
# Locally, use `makepkg -si` instead.
set -euo pipefail

cd "$(dirname "$0")"
root=$(realpath ../..)

# shellcheck disable=SC1091,SC2154 # PKGBUILD only declares variables and functions
mapfile -t deps < <(export startdir=$PWD; source ./PKGBUILD; printf '%s\n' "${depends[@]}" "${makedepends[@]}")
pacman -S --noconfirm --needed "${deps[@]}"

# makepkg refuses to run as root.
id builder > /dev/null 2>&1 || useradd --create-home builder
chown -R builder: "$root"
runuser -u builder -- makepkg --noconfirm
