#!/bin/sh
# Installs a Lighthouse release.
#
# usage: install.sh --version <v0.0.0-...> [--prefix <dir>]
#
# Downloads the archive of this machine and SHA256SUMS from the release,
# checks the sum, unpacks to <prefix>/lib/lighthouse/<version>/ and links
# <prefix>/bin/lighthouse to it. The plugins stay next to the real executable,
# where it finds them through the link. The prefix defaults to ~/.local.
#
# LIGHTHOUSE_DOWNLOAD_URL replaces the release download URL; it is for tests.
set -efu

usage() {
    echo "usage: install.sh --version <v0.0.0-...> [--prefix <dir>]" >&2
    exit 2
}

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

version=
prefix=$HOME/.local
while [ "$#" -gt 0 ]; do
    case $1 in
    --version)
        [ "$#" -ge 2 ] || usage
        version=$2
        shift 2
        ;;
    --prefix)
        [ "$#" -ge 2 ] || usage
        prefix=$2
        shift 2
        ;;
    *) usage ;;
    esac
done
[ -n "$version" ] || usage
tag=v${version#v}
version=${tag#v}

case "$(uname -s)-$(uname -m)" in
Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
Darwin-arm64) target=aarch64-apple-darwin ;;
*) fail "unsupported platform $(uname -s) $(uname -m)" ;;
esac

archive=lighthouse-$version-$target.tar.gz
base=${LIGHTHOUSE_DOWNLOAD_URL:-https://github.com/siyul-park/lighthouse/releases/download}/$tag

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL -o "$work/$archive" "$base/$archive"
curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS"

grep " $archive\$" "$work/SHA256SUMS" >"$work/expected" ||
    fail "SHA256SUMS lists no $archive"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$work" && sha256sum -c expected)
else
    (cd "$work" && shasum -a 256 -c expected)
fi

home=$prefix/lib/lighthouse/$version
rm -rf "$home"
mkdir -p "$home" "$prefix/bin"
tar -xzf "$work/$archive" -C "$home" --strip-components=1
ln -sf "$home/lighthouse" "$prefix/bin/lighthouse"
echo "installed lighthouse $version to $home, linked as $prefix/bin/lighthouse"
