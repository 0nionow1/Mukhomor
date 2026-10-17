"""Collect upstream license notices for the exact locked Arti dependency graph."""
import json
from pathlib import Path
import subprocess
import hashlib
import tomllib
import zipfile
import argparse
from prepare import prepare, pinned_crate, PINS

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--source-only", action="store_true", help="Recreate the pinned source archive without changing notices or pins")
parser.add_argument("--offline", action="store_true")
parser.add_argument("--output", type=Path)
args = parser.parse_args()
if args.output and not args.source_only:
    parser.error("--output requires --source-only")

ROOT = Path(__file__).resolve().parent
prepare(args.offline)
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--manifest-path", str(ROOT / "Cargo.toml"),
    "--format-version", "1", "--locked",
] + (["--offline"] if args.offline else []), text=True))
notices = ["Third-party notices for mukhomor-tor\nDependency versions and checksums: tor/Cargo.lock\n"]
locked = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
checksums = {(p["name"], p["version"]): p.get("checksum") for p in locked["package"]}
destination = args.output or ROOT.parent / "Source" / "tor-dependencies.zip"
source_zip = destination.with_name(destination.name + ".pending")
source_zip.parent.mkdir(parents=True, exist_ok=True)
archive = zipfile.ZipFile(source_zip, "w", zipfile.ZIP_STORED)
canonical = {}
for p in metadata["packages"]:
    directory = Path(p["manifest_path"]).parent
    for kind in ("MIT", "APACHE"):
        candidate = directory / ("LICENSE-" + kind)
        if candidate.is_file() and kind not in canonical:
            canonical[kind] = candidate.read_text(encoding="utf-8")
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["name"] == "mukhomor-tor":
        continue
    folder = Path(package["manifest_path"]).parent
    crate = folder.parent.parent.parent / "cache" / folder.parent.name / (folder.name + ".crate")
    if package["name"] in PINS:
        data = pinned_crate(package["name"], args.offline)
        crate = Path(package["name"] + "-" + package["version"] + ".crate")
        checksums[(package["name"], package["version"])] = PINS[package["name"]][1]
    else:
        data = crate.read_bytes()
    if hashlib.sha256(data).hexdigest() != checksums[(package["name"], package["version"])]:
        raise RuntimeError("Crate checksum mismatch: " + package["name"])
    entry = zipfile.ZipInfo(crate.name, (1980,1,1,0,0,0))
    entry.create_system = 0
    archive.writestr(entry, data)
    if args.source_only:
        continue
    notices.append("\n" + "=" * 72 + "\n" + package["name"] + " " + package["version"])
    notices.append("License: " + (package.get("license") or "See upstream license file"))
    notices.append("Source: https://crates.io/crates/" + package["name"] + "/" + package["version"])
    files = set()
    for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "license*", "licence*", "copyright*"):
        files.update(p for p in folder.glob(pattern) if p.is_file())
    if package.get("license_file"):
        files.add(folder / package["license_file"])
    for subfolder in ("licenses", "LICENSES"):
        if (folder / subfolder).is_dir():
            files.update(p for p in (folder / subfolder).iterdir() if p.is_file())
    if not files:
        license_id = package.get("license") or ""
        notices.append("Authors (upstream metadata): " + ", ".join(package.get("authors", [])))
        notices.append("Copyright and source notices are retained in Source/tor-dependencies.zip, " + crate.name)
        if "MIT" in license_id:
            notices.append("Selected license: MIT\n" + canonical["MIT"][canonical["MIT"].find("Permission is hereby granted"):])
        elif "Apache-2.0" in license_id:
            notices.append("Selected license: Apache-2.0\n" + canonical["APACHE"])
        elif "MPL-2.0" in license_id:
            # priority-queue offers MPL-2.0 as an alternative to LGPL. Its exact
            # unmodified source accompanies every source package.
            notices.append("Selected license: MPL-2.0. Unmodified covered source: " + crate.name + "\n" + (ROOT / "MPL-2.0.txt").read_text(encoding="utf-8"))
        else:
            raise RuntimeError("Review missing license text: " + package["name"])
        for readme in folder.glob("README*"):
            if readme.is_file():
                notices.append("\n--- " + readme.name + " ---\n" + readme.read_text(encoding="utf-8", errors="replace"))
    for path in sorted(files):
        notices.append("\n--- " + path.name + " ---\n" + path.read_text(encoding="utf-8", errors="replace"))
archive.close()
if args.source_only:
    expected = (ROOT / "dependencies.sha256").read_text(encoding="utf-8").split()[0]
    if hashlib.sha256(source_zip.read_bytes()).hexdigest() != expected:
        raise RuntimeError("Recreated Tor source archive differs from the reviewed pin")
    source_zip.replace(destination)
    print("Verified pinned Tor source archive:", destination.name)
    raise SystemExit(0)
(ROOT / "dependencies.sha256").write_text(hashlib.sha256(source_zip.read_bytes()).hexdigest() + "  tor-dependencies.zip\n", encoding="utf-8")
(ROOT / "THIRD_PARTY_NOTICES.txt").write_text("\n".join(notices) + "\n", encoding="utf-8")
source_zip.replace(destination)
print("Collected license notices for", len(metadata["packages"]) - 1, "locked dependencies")
