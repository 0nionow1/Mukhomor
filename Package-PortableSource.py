"""Package reviewed build inputs and the exact prepared core snapshot."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile

ROOT = Path(__file__).resolve().parent
FILES = """
LICENSE Mihomo-LICENSE.txt Slint-LICENSE.txt Rules-LICENSE.txt Tiny5-LICENSE.txt
ThirdParty-LICENSE.txt README.md README.en.md docs/platforms.md
ProfileImport.psm1
Build-All.ps1 Build-Android.ps1 Build-Linux.sh Prepare-PortableCore.py Package-PortableSource.py
go.mod go.sum portable_assets.go
assets/settings.default.json assets/theme.json assets/fonts/Tiny5-Regular.ttf
assets/fonts/README.md lists/sources.json lists/seed-games.list
lists/seed-russia.list lists/seed-steam.list lists/seed-ozon.list
native/Cargo.toml native/Cargo.lock native/build.rs native/visual_build.rs
native/windows.manifest native/README.md
native/src/android.rs native/src/appearance.rs native/src/autostart.rs
native/src/backend.rs native/src/cleanup_request.rs native/src/design.rs
native/src/desktop.rs native/src/hero_motion.rs native/src/i18n.rs
native/src/lib.rs native/src/main.rs native/src/mushroom.rs native/src/platform.rs
native/src/portable.rs native/src/presets.rs native/src/profile_store.rs
native/src/service.rs native/src/setup.rs native/src/setup_recovery.rs
native/src/slint_host.rs native/src/traffic_guard.rs
native/src/traffic_guard_cli.rs native/src/traffic_guard_tun.rs
ui/Cargo.toml ui/Cargo.lock ui/build.rs ui/README.md ui/views/app.slint
ui/src/lib.rs ui/src/state.rs ui/src/preview.rs ui/src/visuals.rs
core/controller.go core/controller_test.go core/import.go core/node_fields.go
core/openvpn.go core/rules.go core/storage.go core/update.go
cmd/mukhomor-android/bridge.c cmd/mukhomor-android/main.go
cmd/mukhomor-controller/main.go
android/AndroidManifest.xml android/res/drawable/ic_mushroom.xml
android/Generate-LaunchArtwork.py
android/res/drawable/ic_mushroom_monochrome.xml android/res/drawable/ic_stat_mushroom.xml
android/res/drawable/startup_background.xml
android/res/drawable/startup_wordmark.xml
android/res/mipmap-anydpi-v26/ic_launcher.xml android/res/mipmap-anydpi-v33/ic_launcher.xml
android/res/values/colors.xml android/res/values/styles.xml android/res/values-v31/styles.xml
android/src/org/mukhomor/app/MainActivity.java
android/src/org/mukhomor/app/MukhomorVpnService.java
android/src/org/mukhomor/app/NativeBridge.java
linux/install.sh linux/uninstall.sh linux/mukhomor.desktop linux/mukhomor.svg
tests/linux-network.py tests/android-fixture.py tests/android-probe.c
tests/Test-PortableReleasePrivacy.py
Source/mihomo-v1.19.32-source.zip
""".split()


def package(platform, output):
    stage = ROOT / "target/portable-core" / platform
    provenance = json.loads((stage / "source.json").read_text(encoding="utf-8"))
    entries = {name: ROOT / name for name in FILES}
    entries["Source/portable-build.json"] = stage / "source.json"
    if provenance["mycelia"]:
        for name in ("mihomo-mycelium-source.zip", "mycelium-build.json"):
            entries["Source/" + name] = stage / name
        if hashlib.sha256((stage / "mihomo-mycelium-source.zip").read_bytes()).hexdigest() != provenance["source_sha256"]:
            raise ValueError("Prepared Mycelia snapshot changed")
    if hashlib.sha256(entries["Source/mihomo-v1.19.32-source.zip"].read_bytes()).hexdigest() != provenance["upstream_sha256"]:
        raise ValueError("Upstream source checksum mismatch")
    version = re.search(r'^version = "([0-9.]+)"', (ROOT / "native/Cargo.toml").read_text(), re.M)[1]
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    manifest = {"version": version, "files": {}}
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name, path in sorted(entries.items()):
            if not path.is_file() or path.is_symlink():
                raise ValueError("Missing or symlinked public build input: " + name)
            content = path.read_bytes()
            archive.writestr("Mukhomor/" + name, content)
            manifest["files"][name] = hashlib.sha256(content).hexdigest()
        archive.writestr("Mukhomor/source-manifest.json", json.dumps(manifest, indent=2) + "\n")
    with zipfile.ZipFile(output) as archive:
        if archive.testzip() is not None:
            raise ValueError("Source archive integrity check failed")
        for name, digest in manifest["files"].items():
            if hashlib.sha256(archive.read("Mukhomor/" + name)).hexdigest() != digest:
                raise ValueError("Source manifest mismatch: " + name)
    output.with_name(output.name + ".sha256").write_text(hashlib.sha256(output.read_bytes()).hexdigest() + "  " + output.name + "\n", encoding="utf-8")
    print("Corresponding portable source:", output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("windows", "linux"), required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    package(args.platform, args.output)
