#!/usr/bin/env bash
# Renders the AUR packages for a published release, test-builds and checks them, and with
# AUR_PUSH=true publishes them. Runs as root inside the pinned archlinux container
# (.github/workflows/aur.yml); git, jq and namcap must be installed, and openssh to publish.
#
#   packaging/aur/publish.sh <vX.Y.Z | latest> [pkgrel]
#
# The templates are packaging/aur/<package>/PKGBUILD. This fills in pkgver, pkgrel and
# sha256sums: ultra-9000-bin's checksum comes from the release's SHA256SUMS.txt,
# ultra-9000's from GitHub's archive of the tag. Each package is built with makepkg,
# checked with namcap, installed, and its binary's shared libraries must all resolve before
# anything is pushed. Publishing refuses to go backwards or to change a published
# pkgver-pkgrel; bump pkgrel to republish packaging changes for the same release.
set -euo pipefail

repo=yasen-pavlov/ultra_9000
packages=(ultra-9000 ultra-9000-bin)
# aur.archlinux.org's ed25519 host key (fingerprint SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4,
# published on https://aur.archlinux.org).
aur_host_key='aur.archlinux.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEuBKrPzbawxA/k2g6NcyV5jmqwJ2s+zpgZGZ7tpLIcN'
aur_name=yasen
aur_email=yasen.pavlov+aur@bitnet.me

cd "$(dirname "$0")"
here=$PWD
root=$(realpath ../..)

tag=${1:?usage: publish.sh <vX.Y.Z|latest> [pkgrel]}
pkgrel=${2:-1}
if [ "$tag" = latest ]; then
  tag=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" | jq -r .tag_name)
fi
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "::error::Not a release tag: '$tag'" >&2; exit 1; }
[[ $pkgrel =~ ^[1-9][0-9]*$ ]] || { echo "::error::pkgrel must be a positive integer: '$pkgrel'" >&2; exit 1; }
version=${tag#v}
echo "AUR packages for $tag (pkgver $version, pkgrel $pkgrel)"

work=$(mktemp -d)
trap 'rm -rf "$work" "$HOME/.ssh/aur"' EXIT

# Publishing: set up and test AUR access before spending minutes on builds.
publish=${AUR_PUSH:-false}
if [ "$publish" = true ]; then
  command -v ssh > /dev/null || { echo "::error::ssh is required to publish (install openssh)" >&2; exit 1; }
  : "${AUR_SSH_PRIVATE_KEY:?AUR_SSH_PRIVATE_KEY is required to publish}"
  install -d -m 700 "$HOME/.ssh"
  printf '%s\n' "$AUR_SSH_PRIVATE_KEY" > "$HOME/.ssh/aur"
  chmod 600 "$HOME/.ssh/aur"
  printf '%s\n' "$aur_host_key" > "$work/known_hosts"
  export GIT_SSH_COMMAND="ssh -i $HOME/.ssh/aur -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=$work/known_hosts"
  for package in "${packages[@]}"; do
    git ls-remote "ssh://aur@aur.archlinux.org/$package.git" > /dev/null
  done
  echo "AUR access works."
fi

# A PKGBUILD array, read the way makepkg does (the PKGBUILDs only declare variables and
# functions at the top level).
pkgbuild_array() { # <PKGBUILD> <array name>
  # shellcheck disable=SC1090
  (
    export startdir
    startdir=$(dirname "$1")
    source "$1"
    declare -n values=$2
    [ "${#values[@]}" -eq 0 ] || printf '%s\n' "${values[@]}"
  )
}

# The AUR packages install what packaging/arch builds, so they need the same runtime deps.
reference=$(pkgbuild_array "$root/packaging/arch/PKGBUILD" depends)
for package in "${packages[@]}"; do
  if [ "$(pkgbuild_array "$here/$package/PKGBUILD" depends)" != "$reference" ]; then
    echo "::error::$package/PKGBUILD depends differ from packaging/arch/PKGBUILD" >&2
    exit 1
  fi
done

asset="ultra-9000-$version-1-x86_64.pkg.tar.zst"
bin_sha=$(curl -fsSL "https://github.com/$repo/releases/download/$tag/SHA256SUMS.txt" \
  | awk -v asset="$asset" '$2 == asset { print $1 }')
[[ $bin_sha =~ ^[0-9a-f]{64}$ ]] || { echo "::error::$asset is not in the release's SHA256SUMS.txt" >&2; exit 1; }
src_sha=$(curl -fsSL "https://github.com/$repo/archive/refs/tags/$tag.tar.gz" | sha256sum | cut -d' ' -f1)
declare -A sha=([ultra-9000]=$src_sha [ultra-9000-bin]=$bin_sha)

id builder > /dev/null 2>&1 || useradd --create-home builder
chown builder: "$work" # mktemp's directory is root-only; makepkg runs as builder
for package in "${packages[@]}"; do
  dir="$work/$package"
  mkdir -p "$dir"
  sed -e "s/^pkgver=.*/pkgver=$version/" -e "s/^pkgrel=.*/pkgrel=$pkgrel/" \
    -e "s/^sha256sums=.*/sha256sums=('${sha[$package]}')/" \
    "$here/$package/PKGBUILD" > "$dir/PKGBUILD"
  grep -qx "sha256sums=('${sha[$package]}')" "$dir/PKGBUILD"
  chown -R builder: "$dir"

  mapfile -t deps < <(pkgbuild_array "$dir/PKGBUILD" depends; pkgbuild_array "$dir/PKGBUILD" makedepends)
  pacman -S --noconfirm --needed "${deps[@]}"

  echo "::group::makepkg $package"
  (cd "$dir" && runuser -u builder -- makepkg --noconfirm)
  echo "::endgroup::"
  (cd "$dir" && runuser -u builder -- makepkg --printsrcinfo > .SRCINFO)

  built="$dir/$package-$version-$pkgrel-x86_64.pkg.tar.zst"
  test -f "$built"
  echo "::group::namcap $package"
  namcap "$dir/PKGBUILD" "$built" | tee "$dir/namcap.txt"
  echo "::endgroup::"
  if grep -q ' E: ' "$dir/namcap.txt"; then
    echo "::error::namcap reports errors for $package" >&2
    exit 1
  fi

  # Install it alone (the two packages conflict) and check that the binary links.
  pacman -U --noconfirm "$built"
  missing=$(ldd /usr/bin/ultra9000 | grep 'not found' || true)
  pacman -R --noconfirm "$package"
  if [ -n "$missing" ]; then
    echo "::error::$package: unresolved shared libraries: $missing" >&2
    exit 1
  fi
done

if [ "$publish" != true ]; then
  echo "Built and checked; not publishing (AUR_PUSH is not true)."
  exit 0
fi

for package in "${packages[@]}"; do
  dir="$work/$package"
  clone="$work/aur-$package"
  new="$version-$pkgrel"
  order=1 # newer than anything published
  git clone --quiet "ssh://aur@aur.archlinux.org/$package.git" "$clone"
  if [ -f "$clone/.SRCINFO" ]; then
    old=$(awk '$1 == "pkgver" { v = $3 } $1 == "pkgrel" { r = $3 } END { print v "-" r }' "$clone/.SRCINFO")
    order=$(vercmp "$new" "$old")
    if [ "$order" -lt 0 ]; then
      echo "::error::$package: the AUR has $old, newer than $new" >&2
      exit 1
    fi
  fi
  cp "$dir/PKGBUILD" "$dir/.SRCINFO" "$clone/"
  git -C "$clone" add PKGBUILD .SRCINFO
  if git -C "$clone" diff --cached --quiet; then
    echo "$package $new is already published."
    continue
  fi
  if [ "$order" -eq 0 ]; then
    echo "::error::$package: $new is published with different contents; bump pkgrel" >&2
    exit 1
  fi
  git -C "$clone" -c user.name="$aur_name" -c user.email="$aur_email" commit --quiet -m "$package $new"
  git -C "$clone" push --quiet origin HEAD:master
  echo "Published $package $new: https://aur.archlinux.org/packages/$package"
done
