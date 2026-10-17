"""Package reviewed build inputs and the exact prepared core snapshot."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
FILES = """
LICENSE Mihomo-LICENSE.txt Slint-LICENSE.txt Rules-LICENSE.txt Tiny5-LICENSE.txt
ThirdParty-LICENSE.txt README.md README.en.md docs/platforms.md
ProfileImport.psm1
Build-All.ps1 Build-Android.ps1 Build-Linux.sh Prepare-PortableCore.py Package-PortableSource.py
go.mod go.sum portable_assets.go
assets/settings.default.json assets/theme.json assets/mushroom-art.json assets/fonts/Tiny5-Regular.ttf
assets/fonts/README.md lists/sources.json lists/seed-games.list
lists/seed-russia.list lists/seed-steam.list lists/seed-ozon.list
native/Cargo.toml native/Cargo.lock native/build.rs native/visual_build.rs
native/windows.manifest native/README.md
native/src/android.rs native/src/appearance.rs native/src/autostart.rs
native/src/backend.rs native/src/cleanup_request.rs native/src/design.rs
native/src/desktop.rs native/src/hero_motion.rs native/src/i18n.rs
native/src/lib.rs native/src/main.rs native/src/mushroom.rs native/src/mushroom_data.rs native/src/platform.rs
native/src/portable.rs native/src/presets.rs native/src/profile_store.rs
native/src/service.rs native/src/setup.rs native/src/setup_recovery.rs
native/src/slint_host.rs native/src/traffic_guard.rs
native/src/app_update.rs
native/src/traffic_guard_cli.rs native/src/traffic_guard_tun.rs
ui/Cargo.toml ui/Cargo.lock ui/build.rs ui/README.md ui/views/app.slint
ui/src/lib.rs ui/src/state.rs ui/src/preview.rs ui/src/visuals.rs
core/controller.go core/controller_test.go core/import.go core/node_fields.go
core/openvpn.go core/rules.go core/storage.go core/update.go core/tor.go core/tor_test.go core/tor_guard_linux.go core/tor_guard_other.go
tor/Cargo.toml tor/Cargo.lock tor/src/main.rs tor/licenses.py tor/prepare.py tor/MPL-2.0.txt tor/THIRD_PARTY_NOTICES.txt tor/dependencies.sha256 Tor-LICENSE.txt docs/tor.md
Source/tor-dependencies.zip
i2p/main.cpp
i2p/CMakeLists.txt
i2p/prepare.py
i2p/build.py
i2p/windows-build.ps1
i2p/sources.py
i2p/THIRD_PARTY_NOTICES.txt
i2p/dependencies.sha256
pt/i2p.go
pt/i2p_test.go
pt/i2p_process_windows.go
pt/i2p_process_other.go
core/i2p_test.go
docs/p2p.md
pt/i2p-reseed/acetone_at_mail.i2p.crt
pt/i2p-reseed/admin_at_likogan.dev.crt
pt/i2p-reseed/admin_at_stormycloud.org.crt
pt/i2p-reseed/creativecowpat_at_mail.i2p.crt
pt/i2p-reseed/diyarciftci_at_protonmail.com.crt
pt/i2p-reseed/echelon3_at_mail.i2p.crt
pt/i2p-reseed/hankhill19580_at_gmail.com.crt
pt/i2p-reseed/i2p-reseed_at_mk16.de.crt
pt/i2p-reseed/igor_at_novg.net.crt
pt/i2p-reseed/lazygravy_at_mail.i2p.crt
pt/i2p-reseed/orignal_at_mail.i2p.crt
pt/i2p-reseed/r4sas-reseed_at_mail.i2p.crt
pt/i2p-reseed/rambler_at_mail.i2p.crt
pt/i2p-reseed/reseed_at_diva.exchange.crt
pt/i2p-reseed/sahil_at_mail.i2p.crt
pt/i2p-reseed/vserod1488_at_proton.me.crt
Source/i2p-dependencies.zip
native/src/tor_options.rs tor/src/bridges.rs
pt/go.mod pt/go.sum pt/main.go pt/main_test.go pt/dns_test.go pt/underlay.go
pt/socket_windows.go pt/socket_linux.go pt/socket_android.go pt/prepare.py pt/licenses.py
pt/THIRD_PARTY_NOTICES.txt pt/dependencies.sha256 Source/pt-dependencies.zip
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
android/src/org/mukhomor/app/UpdateProvider.java
linux/install.sh linux/uninstall.sh linux/mukhomor.desktop linux/mukhomor.svg
tests/linux-network.py tests/tor_fixture.py tests/android-fixture.py tests/android-probe.c
tests/linux-update.py
tests/Test-PortableReleasePrivacy.py
Source/mihomo-v1.19.32-source.zip
""".split()


def package(platform, output, go='go'):
    subprocess.run([sys.executable, str(ROOT / "i2p/sources.py")], check=True)
    if not (ROOT / "Source/pt-dependencies.zip").exists():
        subprocess.run([sys.executable, str(ROOT / "pt/licenses.py"), "--source-only", "--go", go], check=True)
    bridge_expected = (ROOT / "pt/dependencies.sha256").read_text().split()[0]
    if hashlib.sha256((ROOT / "Source/pt-dependencies.zip").read_bytes()).hexdigest() != bridge_expected:
        raise ValueError("Bridge dependency source checksum mismatch")
    if not (ROOT / "Source/tor-dependencies.zip").exists():
        subprocess.run([sys.executable, str(ROOT / "tor/licenses.py"), "--source-only"], check=True)
    expected = (ROOT / "tor/dependencies.sha256").read_text().split()[0]
    if hashlib.sha256((ROOT / "Source/tor-dependencies.zip").read_bytes()).hexdigest() != expected:
        raise ValueError("Tor dependency source checksum mismatch")
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
    parser.add_argument("--go", default="go")
    args = parser.parse_args()
    package(args.platform, args.output, args.go)
