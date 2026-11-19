#!/bin/bash
# Собирает «Яндекс Музыка RS.app» из релизной сборки: Info.plist, значок, ad-hoc подпись.
# Запуск: tools/macos-app.sh [--skip-build]
#
# .app нужен не только для красоты: без бандла macOS не даёт приложению значок в Dock,
# а LaunchAgent автозапуска не может открыть его как приложение.

set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Яндекс Музыка RS"
# Bundle id берём из кода: метка LaunchAgent и Info.plist обязаны совпадать, а копия
# в скрипте рано или поздно разъедется.
BUNDLE_ID=$(grep -m1 'const BUNDLE_ID' src/sys/macos.rs | cut -d'"' -f2)
[ -n "$BUNDLE_ID" ] || { echo "не нашёл BUNDLE_ID в src/sys/macos.rs" >&2; exit 1; }
EXE="yamusic-rs"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT="target/macos"
APP="$OUT/$APP_NAME.app"

if [ "${1:-}" != "--skip-build" ]; then
  cargo build --release
fi
[ -f "target/release/$EXE" ] || { echo "нет target/release/$EXE: сначала cargo build --release" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/release/$EXE" "$APP/Contents/MacOS/$EXE"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$EXE</string>
  <key>CFBundleIconFile</key><string>app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><false/>
  <key>NSHumanReadableCopyright</key><string>GPL-3.0, неофициальный клиент</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# Значок: SVG → 1024 → iconset → icns. rsvg-convert точнее, Chrome есть почти всегда.
MASTER="$OUT/icon-1024.png"
mkdir -p "$OUT"
if command -v rsvg-convert >/dev/null; then
  rsvg-convert -w 1024 -h 1024 icons/app.svg -o "$MASTER"
elif [ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ]; then
  PAGE="$OUT/icon.html"
  cat > "$PAGE" <<HTML
<html><body style="margin:0;background:transparent">
<img src="../../icons/app.svg" style="width:1024px;height:1024px;display:block">
</body></html>
HTML
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu \
    --default-background-color=00000000 --window-size=1024,1024 --hide-scrollbars \
    --screenshot="$PWD/$MASTER" "file://$PWD/$PAGE" >/dev/null 2>&1
  rm -f "$PAGE"
fi

ICONSET="$OUT/app.iconset"
rm -rf "$ICONSET"
if [ -s "$MASTER" ]; then
  mkdir -p "$ICONSET"
  for size in 16 32 64 128 256 512; do
    sips -z $size $size "$MASTER" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double "$MASTER" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/app.icns"
  rm -rf "$ICONSET"
else
  # Без рендера SVG берём готовый 256 px: значок будет мыльным, но будет.
  cp packaging/icons/256.png "$OUT/icon-256.png"
  mkdir -p "$ICONSET"
  for size in 16 32 128 256; do
    sips -z $size $size "$OUT/icon-256.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$OUT/icon-256.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/app.icns"
  rm -rf "$ICONSET"
fi

# Без подписи macOS 15+ не запускает бинарь с Apple Silicon; ad-hoc достаточно.
# Подпись меняется вместе с бинарём, поэтому после обновления Keychain один раз спросит
# пароль за сохранённым токеном.
codesign --force --sign - "$APP" >/dev/null
codesign --verify --deep --strict "$APP"

echo "$APP"
du -sh "$APP" | cut -f1
