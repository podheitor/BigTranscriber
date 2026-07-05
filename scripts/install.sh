#!/usr/bin/env bash
# User-level install (no sudo): puts the binary in ~/.local/bin, installs the
# icon into the hicolor theme, and adds a .desktop launcher so BigTranscriber
# appears in your application menu. Re-run after rebuilding to update.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
APP_ID="bigtranscriber"
BIN_SRC="$ROOT/src-tauri/target/release/bigtranscriber"

[[ -x "$BIN_SRC" ]] || { echo "Binary not built. Run ./scripts/build.sh first." >&2; exit 1; }

# 1) Binary + launcher wrapper.
# The wrapper sets WEBKIT_DISABLE_DMABUF_RENDERER=1 — without it, webkit2gtk on
# NVIDIA can fail GBM buffer allocation and show a blank window.
mkdir -p "$HOME/.local/bin"
install -m755 "$BIN_SRC" "$HOME/.local/bin/$APP_ID-bin"
cat > "$HOME/.local/bin/$APP_ID" <<EOF
#!/usr/bin/env bash
export WEBKIT_DISABLE_DMABUF_RENDERER=1
exec "$HOME/.local/bin/$APP_ID-bin" "\$@"
EOF
chmod +x "$HOME/.local/bin/$APP_ID"

# 2) Icons -> hicolor theme (auto-detect each size)
for src in "$ROOT"/src-tauri/icons/32x32.png "$ROOT"/src-tauri/icons/64x64.png \
           "$ROOT"/src-tauri/icons/128x128.png "$ROOT"/src-tauri/icons/128x128@2x.png \
           "$ROOT"/src-tauri/icons/icon.png; do
  [[ -f "$src" ]] || continue
  wh="$(identify -format '%wx%h' "$src" 2>/dev/null || true)"
  case "$wh" in
    32x32|64x64|128x128|256x256|512x512)
      dir="$HOME/.local/share/icons/hicolor/$wh/apps"
      mkdir -p "$dir"; cp "$src" "$dir/$APP_ID.png" ;;
  esac
done

# 3) Desktop launcher
APPS="$HOME/.local/share/applications"
mkdir -p "$APPS"
cat > "$APPS/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=BigTranscriber
GenericName=Transcrição de audiência
Comment=Transcrição local (offline, GPU) do áudio do sistema + microfone
Exec=$HOME/.local/bin/$APP_ID
Icon=$APP_ID
Terminal=false
Categories=AudioVideo;Audio;
Keywords=transcricao;audiencia;whisper;legenda;audio;
StartupWMClass=BigTranscriber
EOF
chmod +x "$APPS/$APP_ID.desktop"

# 4) Refresh menu/icon caches (best-effort)
update-desktop-database "$APPS" 2>/dev/null || true
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
for k in kbuildsycoca6 kbuildsycoca5; do command -v "$k" >/dev/null && "$k" >/dev/null 2>&1 || true; done

echo "Installed:"
echo "  binary   : $HOME/.local/bin/$APP_ID"
echo "  launcher : $APPS/$APP_ID.desktop"
echo "  icon     : ~/.local/share/icons/hicolor/*/apps/$APP_ID.png"
echo "Search 'BigTranscriber' in your app menu (or run '$APP_ID')."
