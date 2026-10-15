#!/usr/bin/env bash
set -euo pipefail
if [ "$(id -u)" != 0 ]; then echo 'Run: sudo ./install.sh' >&2; exit 1; fi
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
command -v setcap >/dev/null || { echo 'Install libcap2-bin (Debian/Ubuntu) or libcap (Fedora/Arch).' >&2; exit 1; }
command -v zenity >/dev/null || { echo 'Install zenity for profile import and preset file dialogs.' >&2; exit 1; }
test -c /dev/net/tun || { echo '/dev/net/tun is required.' >&2; exit 1; }
for file in mukhomor mukhomor-controller; do test -f "$source_dir/$file" && test ! -L "$source_dir/$file" || exit 1; done
# GUI stays unprivileged. Only the reviewed controller can configure TUN routes.
install -d -m755 /opt/mukhomor
install -m755 "$source_dir/mukhomor" /opt/mukhomor/mukhomor.new
install -m755 "$source_dir/mukhomor-controller" /opt/mukhomor/mukhomor-controller.new
setcap cap_net_admin=ep /opt/mukhomor/mukhomor-controller.new
mv -f /opt/mukhomor/mukhomor.new /opt/mukhomor/mukhomor
mv -f /opt/mukhomor/mukhomor-controller.new /opt/mukhomor/mukhomor-controller
install -Dm644 "$source_dir/mukhomor.desktop" /usr/local/share/applications/mukhomor.desktop
install -Dm644 "$source_dir/mukhomor.svg" /usr/local/share/icons/hicolor/scalable/apps/mukhomor.svg
printf '%s\n' '#!/bin/sh' 'exec /opt/mukhomor/mukhomor "$@"' > /usr/local/bin/mukhomor
chmod 755 /usr/local/bin/mukhomor
echo 'Installed Mukhomor. Run mukhomor as your normal user. File dialogs require zenity.'
