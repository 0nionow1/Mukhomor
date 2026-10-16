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
# Preserve the previous pair, including the controller capability, until every
# installation step succeeds. Profiles live elsewhere and are never touched.
transaction=$(mktemp -d /opt/mukhomor/.update.XXXXXXXX)
committed=0
replacing=0
finish() {
    local status=$?
    trap - EXIT
    if [ "$committed" = 0 ] && [ "$replacing" = 1 ]; then
        for file in mukhomor mukhomor-controller; do
            if [ -f "$transaction/$file.old" ]; then
                mv -f -- "$transaction/$file.old" "/opt/mukhomor/$file" || status=1
            else
                rm -f -- "/opt/mukhomor/$file"
            fi
        done
    fi
    rm -rf -- "$transaction"
    exit "$status"
}
trap finish EXIT
for file in mukhomor mukhomor-controller; do
    if [ -e "/opt/mukhomor/$file" ]; then cp -a -- "/opt/mukhomor/$file" "$transaction/$file.old"; fi
    install -m755 "$source_dir/$file" "$transaction/$file.new"
done
setcap cap_net_admin=ep "$transaction/mukhomor-controller.new"
replacing=1
mv -f -- "$transaction/mukhomor.new" /opt/mukhomor/mukhomor
mv -f -- "$transaction/mukhomor-controller.new" /opt/mukhomor/mukhomor-controller
install -Dm644 "$source_dir/mukhomor.desktop" /usr/local/share/applications/mukhomor.desktop
install -Dm644 "$source_dir/mukhomor.svg" /usr/local/share/icons/hicolor/scalable/apps/mukhomor.svg
printf '%s\n' '#!/bin/sh' 'exec /opt/mukhomor/mukhomor "$@"' > /usr/local/bin/mukhomor
chmod 755 /usr/local/bin/mukhomor
committed=1
echo 'Installed Mukhomor. Run mukhomor as your normal user. File dialogs require zenity.'
