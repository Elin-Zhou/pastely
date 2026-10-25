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

# DMG：app + /Applications 链接（拖拽安装布局）
mkdir -p "$DIST/staging"
cp -R "$APP" "$DIST/staging/"
ln -s /Applications "$DIST/staging/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$DIST/staging" -ov -quiet "$DIST/$APP_NAME-$VERSION.dmg"
rm -rf "$DIST/staging"

echo
echo "打包完成："
echo "  $APP"
echo "  $DIST/$APP_NAME-$VERSION.dmg"
echo "安装：打开 DMG，把 $APP_NAME.app 拖动到 Applications。"
