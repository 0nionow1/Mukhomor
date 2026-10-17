#!/usr/bin/env bash
set -euo pipefail
# Reject an incompatible prebuilt native helper before replacing any files.
glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
glibc=${glibc#glibc }
if [[ ! "$glibc" =~ ^([0-9]+)\.([0-9]+)$ ]] || (( BASH_REMATCH[1] < 2 || (BASH_REMATCH[1] == 2 && BASH_REMATCH[2] < 38) )); then
    echo 'This Linux package requires glibc 2.38 or newer (for example Ubuntu 24.04). No files were installed.' >&2
    exit 1
fi
if [ "$(id -u)" != 0 ]; then echo 'Run: sudo ./install.sh' >&2; exit 1; fi
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
command -v setcap >/dev/null || { echo 'Install libcap2-bin (Debian/Ubuntu) or libcap (Fedora/Arch).' >&2; exit 1; }
command -v zenity >/dev/null || { echo 'Install zenity for profile import and preset file dialogs.' >&2; exit 1; }
test -c /dev/net/tun || { echo '/dev/net/tun is required.' >&2; exit 1; }
for file in mukhomor mukhomor-controller mukhomor-tor mukhomor-pt mukhomor-i2pd; do test -f "$source_dir/$file" && test ! -L "$source_dir/$file" || exit 1; done
# GUI stays unprivileged. The reviewed controller configures TUN routes; the Tor helper can bind its TCP sockets to the physical interface.
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
        for file in mukhomor mukhomor-controller mukhomor-tor mukhomor-pt mukhomor-i2pd; do
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
for file in mukhomor mukhomor-controller mukhomor-tor mukhomor-pt mukhomor-i2pd; do
    if [ -e "/opt/mukhomor/$file" ]; then cp -a -- "/opt/mukhomor/$file" "$transaction/$file.old"; fi
    install -m755 "$source_dir/$file" "$transaction/$file.new"
done
setcap cap_net_raw=ep "$transaction/mukhomor-tor.new"
setcap cap_net_raw=ep "$transaction/mukhomor-pt.new"
setcap cap_net_admin=ep "$transaction/mukhomor-controller.new"
replacing=1
mv -f -- "$transaction/mukhomor-tor.new" /opt/mukhomor/mukhomor-tor
mv -f -- "$transaction/mukhomor-pt.new" /opt/mukhomor/mukhomor-pt
mv -f -- "$transaction/mukhomor-i2pd.new" /opt/mukhomor/mukhomor-i2pd
mv -f -- "$transaction/mukhomor.new" /opt/mukhomor/mukhomor
mv -f -- "$transaction/mukhomor-controller.new" /opt/mukhomor/mukhomor-controller
install -Dm644 "$source_dir/mukhomor.desktop" /usr/local/share/applications/mukhomor.desktop
install -Dm644 "$source_dir/mukhomor.svg" /usr/local/share/icons/hicolor/scalable/apps/mukhomor.svg
printf '%s\n' '#!/bin/sh' 'exec /opt/mukhomor/mukhomor "$@"' > /usr/local/bin/mukhomor
chmod 755 /usr/local/bin/mukhomor
committed=1
echo 'Installed Mukhomor. Run mukhomor as your normal user. File dialogs require zenity.'
