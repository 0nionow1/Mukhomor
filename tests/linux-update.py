"""Exercise the real installer transaction under a temporary filesystem, without root."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = (ROOT / "linux/install.sh").read_text()
for scenario in ("success", "capability_failure", "second_binary_failure"):
    with tempfile.TemporaryDirectory(prefix="mukhomor-upgrade-test-") as temp:
        base = Path(temp)
        package, installed, shims = base / "package", base / "opt/mukhomor", base / "shims"
        for directory in (package, installed, shims, base / "usr/local/bin"):
            directory.mkdir(parents=True, exist_ok=True)
        for name in ("mukhomor", "mukhomor-controller"):
            (package / name).write_text("new-" + name)
            (installed / name).write_text("old-" + name)
        (package / "mukhomor.desktop").write_text("[Desktop Entry]\nName=Mukhomor\n")
        (package / "mukhomor.svg").write_text("<svg/>")
        profile = base / "private/profiles.json"
        profile.parent.mkdir()
        profile.write_text('{"fixture":"preserved"}')
        script = SCRIPT.replace("/opt/mukhomor", str(installed)).replace("/usr/local/", str(base / "usr/local") + "/").replace("/dev/net/tun", "/dev/null")
        (package / "install.sh").write_text(script)
        commands = {"id": "echo 0", "zenity": "exit 0", "setcap": "exit " + ("1" if scenario == "capability_failure" else "0")}
        if scenario == "second_binary_failure":
            commands["mv"] = 'case "$*" in *mukhomor-controller.new*) exit 1;; esac\nexec /usr/bin/mv "$@"'
        for name, body in commands.items():
            path = shims / name
            path.write_text("#!/bin/sh\n" + body + "\n")
            path.chmod(0o755)
        result = subprocess.run(["bash", str(package / "install.sh")], env={**os.environ, "PATH": str(shims) + ":" + os.environ["PATH"]}, capture_output=True, text=True)
        assert (result.returncode == 0) == (scenario == "success"), (scenario, result.stderr)
        for name in ("mukhomor", "mukhomor-controller"):
            assert (installed / name).read_text() == ("new-" if scenario == "success" else "old-") + name, scenario
        assert profile.read_text() == '{"fixture":"preserved"}'
        assert not list(installed.glob(".update.*")), scenario
        print("PASS:", scenario, "— binary pair and profiles preserved")
