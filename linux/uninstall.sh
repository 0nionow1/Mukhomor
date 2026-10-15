#!/usr/bin/env bash
set -euo pipefail
if [ "$(id -u)" != 0 ]; then echo 'Run: sudo ./uninstall.sh' >&2; exit 1; fi
if pgrep -f '^/opt/mukhomor/mukhomor-controller( |$)' >/dev/null || pgrep -x mukhomor >/dev/null; then echo 'Exit Mukhomor before uninstalling.' >&2; exit 1; fi
rm -f -- /opt/mukhomor/mukhomor /opt/mukhomor/mukhomor-controller /usr/local/bin/mukhomor /usr/local/share/applications/mukhomor.desktop /usr/local/share/icons/hicolor/scalable/apps/mukhomor.svg
rmdir /opt/mukhomor 2>/dev/null || true
echo 'Uninstalled. User profiles remain in ~/.local/share/mukhomor (or XDG_DATA_HOME/mukhomor).'
