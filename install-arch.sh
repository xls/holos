#!/bin/sh
# Install hcmd on Arch Linux or Omarchy as a pacman package.
#
#   curl -fsSL https://raw.githubusercontent.com/xls/holos/main/install-arch.sh | sh
#
# Downloads the latest release's Arch package, checks it against the
# release's published SHA256SUMS, and installs it with `sudo pacman -U`, so
# pacman tracks the files and `sudo pacman -R hcmd-bin` removes them. Run it
# again to update.
#
# The package is installed from a downloaded file rather than its URL: pacman
# requires a signature for a package fetched by URL, and this one is checked
# against the release's checksums instead.
set -eu

REPO="xls/holos"
PKG="hcmd-bin-x86_64.pkg.tar.zst"
# HCMD_ARCH_BASE points somewhere else - a mirror, or a test server.
BASE="${HCMD_ARCH_BASE:-https://github.com/${REPO}/releases/latest/download}"

say()  { printf '%s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

have pacman || die "pacman not found - this installer is for Arch Linux and Omarchy; use install.sh elsewhere"
[ "$(uname -m)" = "x86_64" ] || die "the Arch package is x86_64 only; on $(uname -m) use install.sh"
have curl || die "curl not found"
have sha256sum || die "sha256sum not found"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading the latest Holos Commander package"
curl -fsSL "$BASE/$PKG" -o "$tmp/$PKG" || die "could not download $PKG"
curl -fsSL "$BASE/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "could not download SHA256SUMS"

want="$(awk -v f="$PKG" '$2 == f { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$want" ] || die "$PKG is not in the release's SHA256SUMS"
got="$(sha256sum "$tmp/$PKG" | cut -d' ' -f1)"
[ "$want" = "$got" ] || die "checksum mismatch for $PKG - not installing"
say "checksum verified"

# Piped into sh, stdin is this script, so pacman's question is read from the
# terminal instead. With no terminal at all (a CI job), answer it up front.
# In a subshell: a failed redirection on `:` would end this script.
if (: </dev/tty) 2>/dev/null; then
    sudo pacman -U --needed "$tmp/$PKG" </dev/tty
else
    sudo pacman -U --needed --noconfirm "$tmp/$PKG"
fi
say "installed - run hcmd (or holos) to start"
