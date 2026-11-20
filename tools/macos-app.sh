#!/bin/bash
# Собирает «Яндекс Музыка RS.app» из релизной сборки: Info.plist, значок, ad-hoc подпись.
#
#   tools/macos-app.sh                    # нативная сборка (arm64 на Apple Silicon)
#   tools/macos-app.sh --universal        # arm64 + x86_64 одним бинарём (lipo)
#   tools/macos-app.sh --dmg              # ещё и .dmg с ярлыком Applications
#   tools/macos-app.sh --skip-build       # бандл из уже собранных бинарей
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
# CFBundleShortVersionString по требованиям Apple — только цифры и точки: из «2.0.0+upstream.1.1.2»
# в бандл и в имя файла идёт «2.0.0». Полная строка остаётся в Cargo.toml и в описании релиза.
SHORT=$(printf '%s' "$VERSION" | cut -d'+' -f1)
# В системном окне «О программе» macOS печатает «Version <Short> (<Build>)». Кладём в build
# базу апстрима: получается «Version 1.0.2 (upstream 1.1.2)» вместо дубля номера.
UPSTREAM=$(printf '%s' "$VERSION" | sed -n 's/.*+upstream\.//p')
BUILD=${UPSTREAM:+upstream $UPSTREAM}
BUILD=${BUILD:-$SHORT}
OUT="target/macos"
APP="$OUT/$APP_NAME.app"
DMG="$OUT/$EXE-$SHORT-macos.dmg"

UNIVERSAL=0
DMG_ON=0
SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --universal) UNIVERSAL=1 ;;
    --dmg) DMG_ON=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "неизвестный ключ: $arg (см. --help)" >&2; exit 1 ;;
  esac
done

mkdir -p "$OUT"
BIN="target/release/$EXE"
if [ "$UNIVERSAL" -eq 1 ]; then
  TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
  if [ "$SKIP_BUILD" -eq 0 ]; then
    rustup target add "${TARGETS[@]}" >/dev/null
    for target in "${TARGETS[@]}"; do
      cargo build --release --target "$target"
    done
  fi
  for target in "${TARGETS[@]}"; do
    [ -f "target/$target/release/$EXE" ] || { echo "нет target/$target/release/$EXE" >&2; exit 1; }
  done
  BIN="$OUT/$EXE-universal"
  lipo -create "target/aarch64-apple-darwin/release/$EXE" "target/x86_64-apple-darwin/release/$EXE" -output "$BIN"
  lipo -archs "$BIN" | grep -q 'x86_64' || { echo "lipo собрал не universal: $(lipo -archs "$BIN")" >&2; exit 1; }
elif [ "$SKIP_BUILD" -eq 0 ]; then
  cargo build --release
fi
[ -f "$BIN" ] || { echo "нет $BIN: сначала cargo build --release" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$EXE"

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
  <key>CFBundleShortVersionString</key><string>$SHORT</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
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
    sips -z $((size * 2)) $((size * 2)) "$MASTER" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
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
  echo "внимание: SVG не отрендерился, значок собран из готового 256 px" >&2
fi

# Подпись решает две разные задачи. Без неё macOS не запускает бинарь на Apple Silicon.
# Ad-hoc годится для запуска, но личность приложения меняется с каждой сборкой — связка
# ключей тогда спрашивает доступ к токену после каждого обновления. Свой сертификат даёт
# постоянный designated requirement (`identifier + certificate root`), и «Всегда разрешать»
# работает годами. Есть сертификат — подписываем им, нет — ad-hoc.
IDENTITY="${YM_SIGN_IDENTITY:-Yandex Music RS}"
SIGNED=0
if [ -n "${YM_SIGN_KEYCHAIN:-}" ]; then
  codesign --force --sign "$IDENTITY" --keychain "$YM_SIGN_KEYCHAIN" "$APP" >/dev/null 2>&1 && SIGNED=1
else
  codesign --force --sign "$IDENTITY" "$APP" >/dev/null 2>&1 && SIGNED=1
fi
if [ "$SIGNED" -eq 1 ]; then
  echo "подпись: $IDENTITY"
else
  codesign --force --sign - "$APP" >/dev/null
  echo "подпись: ad-hoc (сертификата «$IDENTITY» нет)"
fi
codesign --verify --deep --strict "$APP"

if [ "$DMG_ON" -eq 1 ]; then
  # Ярлык Applications рядом с приложением: обычное перетаскивание при установке.
  STAGE="$OUT/dmg"
  rm -rf "$STAGE"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG"
  hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
  rm -rf "$STAGE"
  hdiutil verify "$DMG" >/dev/null
  echo "$DMG"
  du -sh "$DMG" | cut -f1
fi

echo "$APP"
du -sh "$APP" | cut -f1
