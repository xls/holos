#!/bin/sh
# Bump the AUR package to a release: version, checksums and .SRCINFO.
#
#   packaging/aur/update.sh 0.17.2
#
# The checksums come from the release's own SHA256SUMS, the same file the
# installers verify against, so the package pins exactly what was published.
# Then push packaging/aur/hcmd-bin to the AUR repository (see README there).
set -eu

version="${1:?usage: update.sh <version, e.g. 0.17.2>}"
here="$(cd "$(dirname "$0")/hcmd-bin" && pwd)"
sums="$(curl -fsSL "https://github.com/xls/holos/releases/download/v${version}/SHA256SUMS")"

sum_for() {
    printf '%s\n' "$sums" | awk -v f="hcmd-${version}-$1-unknown-linux-gnu.tar.gz" '$2 == f { print $1 }'
}
x86="$(sum_for x86_64)"
arm="$(sum_for aarch64)"
[ -n "$x86" ] && [ -n "$arm" ] || { echo "no linux-gnu tarballs in v${version}'s SHA256SUMS" >&2; exit 1; }

sed -i \
    -e "s/^pkgver=.*/pkgver=${version}/" \
    -e "s/^pkgrel=.*/pkgrel=1/" \
    -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('${x86}')/" \
    -e "s/^sha256sums_aarch64=.*/sha256sums_aarch64=('${arm}')/" \
    "$here/PKGBUILD"
(cd "$here" && makepkg --printsrcinfo > .SRCINFO)
echo "hcmd-bin ${version}: PKGBUILD and .SRCINFO updated in $here"
