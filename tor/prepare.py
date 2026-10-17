"""Apply the reviewed PT handshake timeout change to checksum-pinned Arti source.

Arti 0.47 has a hard-coded 10-second timeout for the entire PT/TLS handshake.
Snowflake rendezvous and ICE can exceed it. Only that PT timeout becomes 90s;
direct Tor, certificate checks, protocol logic and the outer 6-minute deadline
remain unchanged. The coarse-clock fix terminates saturating-time's limit search
when a sub-tick step makes no progress. The Cargo cache is never modified.
"""
from pathlib import Path
import argparse
import hashlib
import io
import os
import tarfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent
PINS = {
    "tor-chanmgr": ("0.47.0", "431ba53bf020ea8681ef0c2fbf0f0f854997fca9afe1b2c081f41f14ec00cf5d"),
    "saturating-time": ("0.5.0", "a586db154f85f3faf78398c37b6792d71f99d2253d3277571e14f8a8c6e49a65"),
}

def pinned_crate(package, offline=False):
    version, sha256 = PINS[package]
    name = package + "-" + version
    archive = ROOT.parent / "Source/tor-dependencies.zip"
    data = None
    if archive.exists():
        with zipfile.ZipFile(archive) as z:
            data = z.read(name + ".crate")
    if data is None:
        cargo = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
        candidates = list((cargo / "registry/cache").glob("*/" + name + ".crate"))
        if candidates:
            data = candidates[0].read_bytes()
    if data is None:
        if offline:
            raise RuntimeError("Pinned Arti source unavailable offline")
        with urllib.request.urlopen("https://static.crates.io/crates/" + package + "/" + name + ".crate", timeout=60) as r:
            data = r.read(4 * 1024 * 1024)
    if hashlib.sha256(data).hexdigest() != sha256:
        raise RuntimeError("Pinned Arti source checksum mismatch")
    return data

def prepare_crate(package, offline=False):
    dest = ROOT.parent / "target/tor-sources" / package
    name = package + "-" + PINS[package][0]
    with tarfile.open(fileobj=io.BytesIO(pinned_crate(package, offline)), mode="r:gz") as archive:
        for entry in archive:
            if not entry.isfile():
                continue
            path = Path(entry.name)
            if path.parts[0] != name or path.is_absolute() or ".." in path.parts:
                raise RuntimeError("Unsafe Arti source path")
            relative = Path(*path.parts[1:])
            data = archive.extractfile(entry).read()
            if package == "tor-chanmgr" and relative.as_posix() == "src/builder.rs":
                old = b"std::time::Duration::new(10, 0)"
                if data.count(old) != 1:
                    raise RuntimeError("Arti PT timeout patch no longer matches")
                data = data.replace(old, b"std::time::Duration::new(90, 0)")
            if package == "saturating-time" and relative.as_posix() == "src/internal.rs":
                replacements = {
                    b"fn find_max<T: SaturatingTime>()": b"fn find_max<T: SaturatingTime + PartialEq>()",
                    b"fn find_min<T: SaturatingTime>()": b"fn find_min<T: SaturatingTime + PartialEq>()",
                    b"    T: SaturatingTime,": b"    T: SaturatingTime + PartialEq,",
                    b"        let next = f(&res, step);": b"        let next = f(&res, step).filter(|next| *next != res);",
                }
                for old, new in replacements.items():
                    if data.count(old) != 1:
                        raise RuntimeError("Arti coarse-clock patch no longer matches")
                    data = data.replace(old, new)
            output = dest / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            if not output.exists() or output.read_bytes() != data:
                output.write_bytes(data)
def prepare(offline=False):
    for package in PINS:
        prepare_crate(package, offline)
    print("Prepared pinned Arti PT timeout and Windows coarse-clock fixes")

if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--offline", action="store_true")
    prepare(p.parse_args().offline)
