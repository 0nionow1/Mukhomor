#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$repo"
version=$(sed -n 's/^version = "\([0-9.]*\)"$/\1/p' native/Cargo.toml | head -1)
arch=$(uname -m)
case "$arch" in x86_64) suffix=x64;; aarch64) suffix=arm64;; *) echo "Unsupported Linux architecture: $arch" >&2; exit 1;; esac
package_root=$(mktemp -d "${TMPDIR:-/tmp}/mukhomor-package.XXXXXXXX")
trap 'rm -rf -- "$package_root"' EXIT
stage="$package_root/Mukhomor-$version"
output="$repo/release-staging/$version/linux/$suffix"
mkdir -p "$stage" "$output"
prepare=()
if [ "${MUKHOMOR_MYCELIA:-0}" = 1 ]; then prepare+=(--mycelia); fi
python3 Prepare-PortableCore.py "${prepare[@]}"
modfile="$repo/target/portable-core/linux/portable.mod"
cargo build --manifest-path native/Cargo.toml --locked --release --bin mukhomor
go test -modfile="$modfile" -mod=readonly -tags=with_gvisor ./core ./cmd/mukhomor-controller
go build -modfile="$modfile" -mod=readonly -tags=with_gvisor -trimpath -ldflags='-s -w' -o "$stage/mukhomor-controller" ./cmd/mukhomor-controller
target=${CARGO_TARGET_DIR:-"$repo/native/target"}
install -m755 "$target/release/mukhomor" "$stage/mukhomor"
install -m755 linux/install.sh linux/uninstall.sh "$stage/"
install -m644 linux/mukhomor.desktop "$stage/"
install -m644 LICENSE Mihomo-LICENSE.txt Slint-LICENSE.txt Rules-LICENSE.txt Tiny5-LICENSE.txt "$stage/"
install -m644 linux/mukhomor.svg "$stage/mukhomor.svg"
install -m644 docs/platforms.md "$stage/README.md"
mkdir -p "$stage/Source"
install -m644 Source/mihomo-v1.19.32-source.zip Prepare-PortableCore.py "$stage/Source/"
install -m644 target/portable-core/linux/source.json "$stage/Source/build.json"
if [ "${MUKHOMOR_MYCELIA:-0}" = 1 ]; then install -m644 target/portable-core/linux/mihomo-mycelium-source.zip target/portable-core/linux/mycelium-build.json "$stage/Source/"; fi
archive="$package_root/Mukhomor-$version-linux-$suffix.tar.gz"
python3 Package-PortableSource.py --platform linux --output "$stage/Source/Mukhomor-source.zip"
tar --owner=0 --group=0 -C "$(dirname "$stage")" -czf "$archive" "$(basename "$stage")"
(cd "$package_root" && sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256")
install -m644 "$archive" "$output/$(basename "$archive").new"
mv -f "$output/$(basename "$archive").new" "$output/$(basename "$archive")"
install -m644 "$archive.sha256" "$output/$(basename "$archive").sha256"
echo "Linux package: $output/$(basename "$archive")"
