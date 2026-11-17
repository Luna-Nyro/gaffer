#!/bin/bash
# Build and install Gaffer for this user (no root needed).
#   ./install.sh            build + install
#   ./install.sh --remove   uninstall (your show in ~/.config/gaffer is kept)
set -euo pipefail
cd "$(dirname "$0")"
BIN=~/.local/bin/gaffer
APPS=~/.local/share/applications
ICONS=~/.local/share/icons/hicolor/scalable/apps
if [ "${1:-}" = "--remove" ]; then
    [ -x "$BIN" ] && "$BIN" --remove-bar-scenes || true
    rm -f "$BIN" "$APPS/dev.gaffer.Gaffer.desktop" "$ICONS/dev.gaffer.Gaffer.svg"
    if [ -e /etc/udev/rules.d/60-gaffer-udmx.rules ]; then
        sudo -n rm -f /etc/udev/rules.d/60-gaffer-udmx.rules && sudo -n udevadm control --reload || echo "Remove /etc/udev/rules.d/60-gaffer-udmx.rules by hand (needs sudo)"
    fi
    update-desktop-database -q "$APPS" 2>/dev/null || true
    echo "Gaffer removed (your show is still in ~/.config/gaffer)."
    exit 0
fi
cargo build --release -p gaffer
mkdir -p ~/.local/bin "$APPS" "$ICONS"
install -m 755 target/release/gaffer "$BIN.new" && mv -f "$BIN.new" "$BIN"
install -m 644 dist/dev.gaffer.Gaffer.desktop "$APPS/"
install -m 644 dist/dev.gaffer.Gaffer.svg "$ICONS/"
update-desktop-database -q "$APPS" 2>/dev/null || true
gtk-update-icon-cache -q ~/.local/share/icons/hicolor 2>/dev/null || true
# uDMX dongles are plain USB devices: a udev rule lets you use them without root.
if ! cmp -s dist/60-gaffer-udmx.rules /etc/udev/rules.d/60-gaffer-udmx.rules; then
    if sudo -n install -m 644 dist/60-gaffer-udmx.rules /etc/udev/rules.d/ 2>/dev/null && sudo -n udevadm control --reload; then
        echo "Installed the uDMX udev rule"
    else
        echo "Note: for uDMX dongles run: sudo install -m 644 dist/60-gaffer-udmx.rules /etc/udev/rules.d/ && sudo udevadm control --reload"
    fi
fi
# Touch Bar scenes (skipped with a note when barkeep isn't set up).
"$BIN" --install-bar-scenes || echo "Touch Bar scenes skipped"
id -nG | grep -qw dialout || echo "Note: add yourself to the dialout group to use USB-DMX interfaces: sudo usermod -aG dialout $USER (then log in again)"
echo "Installed: $BIN (menu: Gaffer)"
