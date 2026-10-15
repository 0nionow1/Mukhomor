"""Reject machine paths and credentials in portable runtime packages."""
import argparse
import hashlib
from pathlib import PurePosixPath
import re
import tarfile
import unittest
from unittest.mock import patch
import zipfile

BUILD_PATH = re.compile(
    rb"(?i)(?:[A-Z]:[/\\](?:VPS[/\\]|Users[/\\](?!Public\b|Default\b|<))"
    rb"|/mnt/[a-z]/VPS/|/(?:home|Users)/(?!<)[^/\s\x00]+/)"
)
CREDENTIALS = {
    "GitHub credential": re.compile(rb"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{50,})\b"),
    "private key block": re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\r\n]+[A-Za-z0-9+/=\r\n]{64,}"),
}
# Public build provenance in EasyTier's precompiled Rust FFI, pinned by go.sum:
# easytier-go v0.0.0-20260910071355-3d0c9c3ca5e2. Match only its exact
# upstream home prefix and Rust dependency caches in the Go core binaries.
UPSTREAM_HOME_PREFIX_SHA256 = "9654c9603ab1779404549fb528c883c521494a8a9193c2e193f8d59da9ca50ae"
UPSTREAM_RUST_CACHES = (b".cargo/registry/src/", b".cargo/git/checkouts/", b".rustup/toolchains/")


def is_public_upstream_path(name, data, match):
    return (
        PurePosixPath(name).name in ("libmukhomor_core.so", "mukhomor-controller")
        and match.group().startswith(b"/home/")
        and hashlib.sha256(match.group()).hexdigest() == UPSTREAM_HOME_PREFIX_SHA256
        and data[match.end():].startswith(UPSTREAM_RUST_CACHES)
    )


def check_bytes(name, data):
    for match in BUILD_PATH.finditer(data):
        if not is_public_upstream_path(name, data, match):
            raise ValueError("Local build path in " + name)
    for label, pattern in CREDENTIALS.items():
        if pattern.search(data):
            raise ValueError(label + " in " + name)


def check_name(name):
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or "\\" in name:
        raise ValueError("Unsafe archive path")
    if re.search(r"(?i)(^|/)(private|runtime|\.git)/|\.(p12|pfx|jks|keystore|db|log)$", name):
        raise ValueError("Private archive path")


def check_archive(archive):
    count = 0
    if str(archive).endswith(".apk"):
        with zipfile.ZipFile(archive) as package:
            if package.testzip() is not None:
                raise ValueError("APK integrity failure")
            expected = {"lib/" + abi + "/" + lib for abi in ("arm64-v8a", "x86_64")
                        for lib in ("libmukhomor_core.so", "libmukhomor_app.so")}
            libraries = {item.filename for item in package.infolist() if item.filename.endswith(".so")}
            if not libraries or not libraries.issubset(expected):
                raise ValueError("Unexpected native library layout")
            for item in package.infolist():
                check_name(item.filename)
                if item.filename.endswith(".so"):
                    check_bytes(item.filename, package.read(item))
                    count += 1
    else:
        with tarfile.open(archive, "r:gz") as package:
            for item in package.getmembers():
                check_name(item.name)
                if item.issym() or item.islnk():
                    raise ValueError("Link in runtime package")
                if item.isfile() and PurePosixPath(item.name).name in ("mukhomor", "mukhomor-controller"):
                    check_bytes(item.name, package.extractfile(item).read())
                    count += 1
            if count != 2:
                raise ValueError("Missing Linux executables")
    print("PASS: portable release privacy; " + str(count) + " native binaries checked")


class PrivacyTests(unittest.TestCase):
    def test_public_upstream_exception_is_limited_to_pinned_core_cache_paths(self):
        prefix = b"/home/upstream-fixture/"
        with patch(__name__ + ".UPSTREAM_HOME_PREFIX_SHA256", hashlib.sha256(prefix).hexdigest()):
            for name in ("lib/arm64-v8a/libmukhomor_core.so", "package/mukhomor-controller"):
                for cache in UPSTREAM_RUST_CACHES:
                    check_bytes(name, prefix + cache + b"fixture.rs")
                for value in (prefix + b"private/config", b"/home/other/.cargo/registry/src/file.rs"):
                    with self.assertRaises(ValueError):
                        check_bytes(name, value)
            for name in ("lib/arm64-v8a/libmukhomor_app.so", "package/mukhomor"):
                with self.assertRaises(ValueError):
                    check_bytes(name, prefix + UPSTREAM_RUST_CACHES[0] + b"file.rs")

    def test_machine_paths_are_rejected_in_binary_bytes(self):
        for path in (b"C:\\Users\\example\\.cargo\\src", b"Z:/VPS/project/main.go",
                     b"/mnt/q/VPS/project/main.rs", b"/home/example/.cargo/src"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                check_bytes("fixture.so", b"\x7fELF\x00" + path + b"\x00")

    def test_remapped_paths_and_standard_runtime_paths_are_allowed(self):
        check_bytes("fixture.so", b"/project/ui/src/lib.rs\x00/build-user/.cargo/src\x00/usr/lib/libc.so\x00/data/user/0/org.example")

    def test_credentials_are_rejected_without_disclosing_values(self):
        value = b"ghp_" + b"A" * 36
        with self.assertRaisesRegex(ValueError, "^GitHub credential in fixture.so$"):
            check_bytes("fixture.so", value)

    def test_archive_traversal_and_signing_material_are_rejected(self):
        for name in ("../outside", "/outside", "package/private/password.xml", "package/release.p12"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                check_name(name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=["privacy"], exit=True)
    elif args.archive:
        check_archive(args.archive)
    else:
        parser.error("Pass --archive or --self-test")
