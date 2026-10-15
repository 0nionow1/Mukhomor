"""Reject machine paths and credentials in portable runtime packages."""
import argparse
from pathlib import PurePosixPath
import re
import tarfile
import unittest
import zipfile

BUILD_PATH = re.compile(
    rb"(?i)(?:[A-Z]:[/\\](?:VPS[/\\]|Users[/\\](?!Public\b|Default\b|<))"
    rb"|/mnt/[a-z]/VPS/|/(?:home|Users)/(?!<)[^/\s\x00]+/)"
)
CREDENTIALS = {
    "GitHub credential": re.compile(rb"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{50,})\b"),
    "private key block": re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\r\n]+[A-Za-z0-9+/=\r\n]{64,}"),
}


def check_bytes(name, data):
    if BUILD_PATH.search(data):
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
