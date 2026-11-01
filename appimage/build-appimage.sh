#!/usr/bin/env bash
# Plotter Controller (desktop) -> single-file AppImage (Linux x86_64). No Node.js needed at runtime.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; OUT="$ROOT/release"; B="$ROOT/.appimage-build"; A="$B/AppDir"
rm -rf "$B"; mkdir -p "$A/usr/bin" "$OUT"

echo "==> 1/4 UI (vite build -> ui/dist, embedded into the binary)"
( cd "$ROOT/ui" && { [ -d node_modules ] || npm install --no-audit --no-fund; } && npx vite build )

echo "==> 2/4 cargo build --release"
( cd "$ROOT" && cargo build --release )
install -m755 "$ROOT/target/release/plotter-controller" "$A/usr/bin/plotter-controller"

echo "==> 3/4 AppDir"
cat > "$A/AppRun" <<'RUN'
#!/bin/sh
H="$(dirname "$(readlink -f "$0")")"
if ! ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1.so.0; then
  echo "Missing runtime library libwebkit2gtk-4.1-0 (WebKitGTK 4.1)." >&2
  echo "Install it and run again:  sudo apt install libwebkit2gtk-4.1-0   (Debian/Ubuntu)" >&2
  echo "Не хватает библиотеки libwebkit2gtk-4.1-0. Установите:  sudo apt install libwebkit2gtk-4.1-0" >&2
  exit 1
fi
id -nG 2>/dev/null | tr ' ' '\n' | grep -qx dialout || echo "Note: for serial access add yourself to the 'dialout' group: sudo usermod -aG dialout \$USER (then re-login)" >&2
exec "$H/usr/bin/plotter-controller" "$@"
RUN
chmod +x "$A/AppRun"
cat > "$A/plotter-controller.desktop" <<'DESK'
[Desktop Entry]
Type=Application
Name=Plotter Controller
Name[ru]=Управление плоттером
Name[et]=Plotteri juhtimine
Exec=AppRun
Icon=plotter-controller
Categories=Utility;Electronics;
Terminal=false
DESK
cat > "$A/plotter-controller.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128"><rect width="128" height="128" rx="22" fill="#1f6feb"/><path d="M24 96 L56 40 L72 68 L104 28" fill="none" stroke="#fff" stroke-width="8" stroke-linecap="round" stroke-linejoin="round"/><circle cx="104" cy="28" r="8" fill="#fff"/></svg>
SVG
# appimagetool requires a real raster icon: render the same artwork as PNG + .DirIcon
python3 - "$A" <<'PYEOF'
import sys
from PIL import Image, ImageDraw
img = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle([4, 4, 252, 252], radius=44, fill=(31, 111, 235, 255))
d.line([(48, 192), (112, 80), (144, 136), (208, 56)], fill=(255, 255, 255, 255), width=16, joint="curve")
d.ellipse([192, 40, 224, 72], fill=(255, 255, 255, 255))
img.save(sys.argv[1] + "/plotter-controller.png")
PYEOF
ln -sf plotter-controller.png "$A/.DirIcon"

echo "==> 4/4 appimagetool"
TOOL="$B/appimagetool"
[ -x "$TOOL" ] || { curl -fL -o "$TOOL" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage; chmod +x "$TOOL"; }
VER="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$A" "$OUT/Plotter-Controller-$VER-x86_64.AppImage"
ls -lh "$OUT"
