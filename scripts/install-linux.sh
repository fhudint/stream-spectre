#!/usr/bin/env bash
# Builds stream-spectre and installs it for the current user on Linux:
#
#   ~/.local/bin/stream-spectre         the program, for a terminal (no window)
#   ~/.local/bin/stream-spectre-window  the same program with its dashboard in a window
#   ~/.local/share/applications/stream-spectre-window.desktop
#                                       the entry "Stream Spectre" in the app menu
#   ~/.local/share/icons/hicolor/*/apps/stream-spectre.*   the ghost icon
#
# The menu entry opens the window; closing the window stops the program, after
# it has saved. Building the window needs the development files of GTK 3 and
# WebKitGTK (Fedora: sudo dnf install webkit2gtk4.1-devel gtk3-devel
# libsoup3-devel). On the first install the config files found in this
# repository (config.toml, .env, ...) are copied to ~/.config/stream-spectre/,
# where the program looks when started from the menu; nothing is overwritten.
#
#   scripts/install-linux.sh              build and install
#   scripts/install-linux.sh --no-build   install the binary already built
#   scripts/install-linux.sh --uninstall  remove the program, the entry and the icons
#                                         (the config in ~/.config is kept)
#
# PREFIX (default ~/.local) and XDG_CONFIG_HOME can be set to try it elsewhere.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
prefix="${PREFIX:-$HOME/.local}"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/stream-spectre"
bin="$prefix/bin/stream-spectre"
bin_window="$prefix/bin/stream-spectre-window"
desktop="$prefix/share/applications/stream-spectre-window.desktop"
old_desktop="$prefix/share/applications/stream-spectre.desktop"
icons="$prefix/share/icons/hicolor"
sizes="16 32 48 64 128 256 512"

refresh_caches() {
  command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -f -t "$icons" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$bin" "$bin_window" "$desktop" "$old_desktop" "$icons/scalable/apps/stream-spectre.svg"
  for n in $sizes; do rm -f "$icons/${n}x${n}/apps/stream-spectre.png"; done
  refresh_caches
  echo "Removed. Your config in $config_dir was kept."
  exit 0
fi

if [ "${1:-}" != "--no-build" ]; then
  command -v pkg-config >/dev/null && pkg-config --exists webkit2gtk-4.1 gtk+-3.0 || {
    echo "The window needs the development files of GTK 3 and WebKitGTK." >&2
    echo "Fedora: sudo dnf install webkit2gtk4.1-devel gtk3-devel libsoup3-devel" >&2
    exit 1
  }
  (cd "$repo" && cargo build --release --locked --features window)
fi
for name in stream-spectre stream-spectre-window; do
  [ -x "$repo/target/release/$name" ] || { echo "No binary at $repo/target/release/$name: build first." >&2; exit 1; }
done

install -Dm755 "$repo/target/release/stream-spectre" "$bin"
install -Dm755 "$repo/target/release/stream-spectre-window" "$bin_window"

for n in $sizes; do
  install -Dm644 "$repo/assets/icon-$n.png" "$icons/${n}x${n}/apps/stream-spectre.png"
done
install -Dm644 "$repo/assets/icon.svg" "$icons/scalable/apps/stream-spectre.svg"

mkdir -p "$(dirname "$desktop")"
rm -f "$old_desktop" # an entry of an earlier version of this script
cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Stream Spectre
Comment=Chat-driven mediashare overlay for OBS
Exec=$bin_window
Icon=stream-spectre
Terminal=false
Categories=AudioVideo;Video;
StartupWMClass=stream-spectre-window
EOF
chmod 644 "$desktop"
command -v desktop-file-validate >/dev/null && desktop-file-validate "$desktop"
refresh_caches

# First install only: bring the config along, so the menu entry starts with
# the same settings, API key and history as a run from the repository.
if [ ! -e "$config_dir/config.toml" ]; then
  mkdir -p "$config_dir"
  chmod 700 "$config_dir"
  source_config="$repo/config.toml"
  [ -f "$source_config" ] || source_config="$repo/config.example.toml"
  cp -p "$source_config" "$config_dir/config.toml"
  echo "Config: $source_config -> $config_dir/config.toml"
  for name in config.settings.toml config.history.json config.secrets.toml .env; do
    if [ -f "$repo/$name" ]; then
      cp -p "$repo/$name" "$config_dir/$name"
      echo "Copied $name"
    fi
  done
else
  echo "Config in $config_dir already exists: left as it is."
fi

echo
echo "Installed: $bin_window (and $bin, for a terminal)"
echo "Find \"Stream Spectre\" in your application menu (log out and in if the icon is not there yet)."
case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) echo "Note: $prefix/bin is not in your PATH; the menu entry works anyway." ;;
esac
