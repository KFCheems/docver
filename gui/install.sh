#!/usr/bin/env bash
# Install the GUI beside the CLI. Re-run after `cargo build --release`.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bin="${HOME}/.local/bin"
app="${HOME}/.local/share/applications"
icon="${HOME}/.local/share/icons/hicolor"

mkdir -p "$bin" "$app" "$icon/256x256/apps"
install -m755 "$here/target/release/docver-gui" "$bin/docver-gui"
install -m644 "$here/assets/docver.png" "$icon/256x256/apps/docver.png"
install -m644 "$here/assets/docver.desktop" "$app/docver.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$app" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qt "$icon" 2>/dev/null || true
echo "installed: $bin/docver-gui"
