"""Prepare pinned corresponding source and explicit Android FD ownership.

Writes only target/portable-core/<host>; never modifies the Go module cache.
The optional reviewed Mycelia source bundle uses the same provenance as Windows.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent
PIN = "135f16d1e309f8c0ce2303c86fd38a38624a229e7185ceb74d1d3a4f0d4b66f1"


def unpack(archive, destination):
    with zipfile.ZipFile(archive) as source:
        for item in source.infolist():
            path = PurePosixPath(item.filename)
            if path.is_absolute() or ".." in path.parts or "\\" in item.filename:
                raise ValueError("Unsafe source archive path")
            if "vendor" in path.parts or item.is_dir():
                continue
            output = destination.joinpath(*path.parts)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(source.read(item))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--go", default="go")
    parser.add_argument("--mycelia", action="store_true", help="Require the reviewed experimental source bundle")
    args = parser.parse_args()
    stage = ROOT / "target/portable-core" / ("windows" if os.name == "nt" else "linux")
    stage.mkdir(parents=True, exist_ok=True)
    # Rust build directories can contain hundreds of thousands of files, with
    # temporary paths disappearing during compilation. Exclude them from Go's
    # module discovery without touching either language's source tree.
    for build in [ROOT / "target", ROOT / "native/target", ROOT / "ui/target"]:
        build.mkdir(parents=True, exist_ok=True)
        boundary = build / "go.mod"
        if not boundary.exists():
            boundary.write_text("module mukhomor.invalid/build-artifacts\n\ngo 1.27.2\n", newline="\n")
    archive = ROOT / "Source/mihomo-v1.19.32-source.zip"
    if not archive.exists():
        archive.parent.mkdir(exist_ok=True)
        temporary = stage / "upstream.download"
        with urllib.request.urlopen("https://codeload.github.com/MetaCubeX/mihomo/zip/refs/tags/v1.19.32", timeout=90) as response:
            temporary.write_bytes(response.read(64 * 1024 * 1024))
        if hashlib.sha256(temporary.read_bytes()).hexdigest() != PIN:
            raise ValueError("Upstream source checksum mismatch")
        shutil.copyfile(temporary, archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != PIN:
        raise ValueError("Upstream source checksum mismatch")
    custom = ROOT / "Source/mihomo-mycelium-source.zip"
    provenance = ROOT / "Source/mycelium-build.json"
    identity = PIN
    if args.mycelia:
        if (ROOT / "bin/mycelium-build.json").exists():
            provenance = ROOT / "bin/mycelium-build.json"
        manifest = json.loads(provenance.read_text(encoding="utf-8-sig"))
        identity = hashlib.sha256(custom.read_bytes()).hexdigest()
        if manifest["schema"] != 1 or manifest["protocol_version"] != 1 or manifest["base_version"] != "1.19.32" or identity != manifest["source_sha256"]:
            raise ValueError("Mycelia source provenance mismatch")
    destination = Path(os.environ.get("MUKHOMOR_CORE_CACHE", str(stage))) / identity
    marker = destination / ".extracted"
    if not marker.exists():
        unpack(custom if args.mycelia else archive, destination)
        marker.write_text(identity)
    base = destination / "Mukhomor-core" if args.mycelia else destination
    upstream = base / "upstream/mihomo-1.19.32" if args.mycelia else base / "mihomo-1.19.32"
    # Android's VpnService retains its FD. Duplicate only at the point where
    # sing-tun takes ownership, so parse/permission/cancellation failures leak
    # nothing and neither Java nor Go can double-close a recycled descriptor.
    factory = upstream / "listener/sing_tun/server_notwindows.go"
    factory.write_text(factory.read_text().replace("//go:build !windows\n", "//go:build !windows && !android\n"), newline="\n")
    # Preserve upstream's Android application/package routing implementation.
    original = ("Mukhomor-core/upstream/mihomo-1.19.32/" if args.mycelia else "mihomo-1.19.32/") + "listener/sing_tun/server_android.go"
    with zipfile.ZipFile(custom if args.mycelia else archive) as source:
        android_source = source.read(original).decode("utf-8")
    hook = "func (l *Listener) buildAndroidRules(tunOptions *tun.Options) error {"
    if android_source.count(hook) != 1:
        raise ValueError("Upstream Android rule hook changed")
    android_source = android_source.replace(hook, hook + '''
    // An unprivileged VpnService configures package exclusions through Builder.
    // A borrowed descriptor never needs the root-only packages.xml reader.
    if tunOptions.FileDescriptor > 0 && !tunOptions.AutoRoute { return nil }
''')
    (upstream / "listener/sing_tun/server_android.go").write_text(android_source, newline="\n")
    (upstream / "listener/sing_tun/server_android_fd.go").write_text('''//go:build android

package sing_tun

import (
    "errors"
    tun "github.com/metacubex/sing-tun"
    "golang.org/x/sys/unix"
)

func tunNew(options tun.Options) (tun.Tun, error) {
    if options.FileDescriptor <= 0 { return nil, errors.New("Android VPN descriptor is required") }
    fd, err := unix.FcntlInt(uintptr(options.FileDescriptor), unix.F_DUPFD_CLOEXEC, 0)
    if err != nil { return nil, err }
    options.FileDescriptor = fd
    device, err := tun.New(options)
    if err != nil { _ = unix.Close(fd) }
    return device, err
}
''', newline="\n")
    mod = stage / "portable.mod"
    text = (ROOT / "go.mod").read_text()
    text += f'\nreplace github.com/metacubex/mihomo => "{upstream.as_posix()}"\n'
    # Keep upstream's explicit protobuf replacement in the main module.
    text += "replace google.golang.org/protobuf => github.com/metacubex/protobuf-go v0.0.0-20260306035419-7ceee0674686\n"
    if args.mycelia:
        text += f'replace github.com/0nionow1/mycelium => "{base.as_posix()}"\n'
        text += f'replace github.com/metacubex/utls => "{(base / "third_party/utls").as_posix()}"\n'
    mod.write_text(text, newline="\n")
    shutil.copyfile(ROOT / "go.sum", mod.with_suffix(".sum"))
    subprocess.run([args.go, "mod", "tidy", "-modfile=" + str(mod)], cwd=ROOT, check=True)
    (stage / "source.json").write_text(json.dumps({"upstream_sha256": PIN, "mycelia": args.mycelia, "source_sha256": identity, "android_patch": "vpn-service-fd-and-packages-v2"}, indent=2))
    if args.mycelia:
        shutil.copyfile(provenance, stage / "mycelium-build.json")
        shutil.copyfile(custom, stage / "mihomo-mycelium-source.zip")
    print("Prepared portable core:", mod)


if __name__ == "__main__":
    main()
