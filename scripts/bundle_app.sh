#!/usr/bin/env bash
set -euo pipefail

# Pastely 打包：release 构建 → Pastely.app（Info.plist + 图标）→ ad-hoc 签名 → DMG
cd "$(dirname "$0")/.."

APP_NAME="Pastely"
BUNDLE_ID="com.xelin.pastely"
VERSION="0.1.0"
DIST="dist"
APP="$DIST/$APP_NAME.app"

source ~/.cargo/env
cargo build --release

# 组装 .app 目录结构
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/cliphub "$APP/Contents/MacOS/$APP_NAME"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleDisplayName</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key><string>$APP_NAME</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
</dict>
</plist>
PLIST

# 图标：gen_icon.py 画 1024 母版 → sips 缩放 iconset → iconutil 合 icns。
# 任何一步失败都不阻断打包（跳过图标，使用系统默认图标）
ICONSET="$DIST/AppIcon.iconset"
mkdir -p "$ICONSET"
if python3 scripts/gen_icon.py "$ICONSET/icon_1024x1024.png"; then
    icon_ok=1
    scale_icon() { # <目标文件名> <边长>
        sips -z "$2" "$2" "$ICONSET/icon_1024x1024.png" --out "$ICONSET/$1" >/dev/null || icon_ok=0
    }
    scale_icon "icon_16x16.png" 16
    scale_icon "icon_16x16@2x.png" 32
    scale_icon "icon_32x32.png" 32
    scale_icon "icon_32x32@2x.png" 64
    scale_icon "icon_128x128.png" 128
    scale_icon "icon_128x128@2x.png" 256
    scale_icon "icon_256x256.png" 256
    scale_icon "icon_256x256@2x.png" 512
    scale_icon "icon_512x512.png" 512
    cp "$ICONSET/icon_1024x1024.png" "$ICONSET/icon_512x512@2x.png"
    if [ "$icon_ok" -eq 1 ] && iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"; then
        echo "图标生成完成：$APP/Contents/Resources/AppIcon.icns"
    else
        echo "警告：图标合成失败，跳过（使用系统默认图标）" >&2
    fi
    rm -rf "$ICONSET"
else
    echo "警告：图标绘制失败，跳过（使用系统默认图标）" >&2
fi

# ad-hoc 签名（本地运行免 Gatekeeper 阻断；分发需开发者证书另签）
codesign --force -s - "$APP"

# DMG 设计化：AppKit 渲染背景图 → 可读写临时 DMG + Finder 图标布局 → 压缩只读成品
BG_DIR="$DIST/.background_tmp"
mkdir -p "$BG_DIR"
cargo run --release --bin gen_dmg_bg -- "$BG_DIR/dmg_bg.png" 1
cargo run --release --bin gen_dmg_bg -- "$BG_DIR/dmg_bg@2x.png" 2

rm -f "$DIST/tmp_rw.dmg"
# 挂载点被占（用户开着旧 DMG）时，临时卷改用 -build 后缀名
MNT="/Volumes/$APP_NAME"
TEMP_VOL="$APP_NAME"
if [ -e "$MNT" ]; then
    echo "提示：$MNT 已被占用（旧 DMG 可能仍打开），临时卷改用 $APP_NAME-build" >&2
    TEMP_VOL="$APP_NAME-build"
    MNT="/Volumes/$TEMP_VOL"
    hdiutil detach "$MNT" -force >/dev/null 2>&1 || true
fi
hdiutil create -size 64m -fs HFS+J -volname "$TEMP_VOL" -ov -quiet "$DIST/tmp_rw.dmg"
hdiutil attach "$DIST/tmp_rw.dmg" -mountpoint "$MNT" -nobrowse -quiet

cp -R "$APP" "$MNT/"
ln -s /Applications "$MNT/Applications"
mkdir -p "$MNT/.background"
# 多表示 TIFF（1x + 2x 合一）：Finder 在 Retina 屏自动选用 2x 层（LightCraft 同款方案）
tiffutil -cathidpicheck "$BG_DIR/dmg_bg.png" "$BG_DIR/dmg_bg@2x.png" -out "$BG_DIR/background.tiff" >/dev/null
cp "$BG_DIR/background.tiff" "$MNT/.background/background.tiff"

# Finder 布局：窗口尺寸对应背景图，图标摆在右区中部（避开标题与底部提示）
# osascript 失败不中断打包（退化为普通 DMG 布局）
osascript <<APPLESCRIPT || echo "警告：Finder 布局失败，退化为普通 DMG 布局" >&2
tell application "Finder"
  tell disk "$TEMP_VOL"
    open
    delay 2
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {100, 100, 980, 660}
    set viewOptions to icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 96
    set text size of viewOptions to 15
    set label position of viewOptions to bottom
    set background picture of viewOptions to file ".background:background.tiff"
    -- position 在窗口完成加载后设置才可靠；连续设置两次是社区针对失效的标准做法
    delay 1
    set position of item "$APP_NAME.app" of container window to {432, 274}
    set position of item "Applications" of container window to {632, 274}
    delay 1
    set position of item "$APP_NAME.app" of container window to {432, 274}
    set position of item "Applications" of container window to {632, 274}
    close
    delay 1
    open
    update without registering applications
  end tell
end tell
APPLESCRIPT
sleep 2
# 关闭窗口促使 Finder 尽早落盘 .DS_Store（图标摆位/背景引用都在其中）
osascript -e 'tell application "Finder" to close every window' 2>/dev/null || true
sleep 2

# 卸载：先关全部 Finder 窗口释放占用，普通卸载多次重试，最后才强卸
osascript -e 'tell application "Finder" to close every window' 2>/dev/null || true
DETACHED=0
for i in 1 2 3 4 5; do
    sleep 1
    if hdiutil detach "$MNT" >/dev/null 2>&1; then DETACHED=1; break; fi
done
if [ "$DETACHED" -ne 1 ]; then
    echo "WARN: cannot detach $MNT, keep tmp dmg at $DIST/tmp_rw.dmg" >&2
    exit 1
fi
hdiutil convert "$DIST/tmp_rw.dmg" -format UDZO -o "$DIST/$APP_NAME-$VERSION.dmg" -ov -quiet
rm -f "$DIST/tmp_rw.dmg"
rm -rf "$BG_DIR"

echo
echo "打包完成："
echo "  $APP"
echo "  $DIST/$APP_NAME-$VERSION.dmg"
echo "安装：打开 DMG，把 $APP_NAME.app 拖动到 Applications。"
