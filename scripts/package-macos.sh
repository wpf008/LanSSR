#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

if [ "$(uname -s)" != Darwin ]; then
    echo "This packaging script requires macOS." >&2
    exit 1
fi

cargo build --locked --release --bins
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
architecture=$(lipo -archs target/release/LanSender)
case "$architecture" in
    arm64) archive_arch=arm64; architecture_description='Apple Silicon' ;;
    x86_64) archive_arch=x64; architecture_description='Intel' ;;
    *) echo "Unsupported macOS architecture: $architecture" >&2; exit 1 ;;
esac
[ "$(lipo -archs target/release/LanReceiver)" = "$architecture" ]
minimum_os=$(otool -l target/release/LanSender | awk '
    /cmd LC_BUILD_VERSION/ { build = 1 }
    build && $1 == "minos" { print $2; exit }
    /cmd LC_VERSION_MIN_MACOSX/ { legacy = 1 }
    legacy && $1 == "version" { print $2; exit }
')
[ -n "$minimum_os" ] || { echo "Cannot detect minimum macOS version." >&2; exit 1; }
sender_bundle='dist/LanSender.app'
receiver_bundle='dist/LanReceiver.app'
for bundle in "$sender_bundle" "$receiver_bundle"; do
    mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
    rm -f "$bundle/Contents/Resources/conf/imgBridge.properties"
    rmdir "$bundle/Contents/Resources/conf" 2>/dev/null || true
    cp LICENSE "$bundle/Contents/Resources/LICENSE"
    cp vendor/rfd/LICENSE "$bundle/Contents/Resources/rfd-LICENSE"
    cp vendor/egui_glow/LICENSE-MIT "$bundle/Contents/Resources/egui_glow-LICENSE"
    cat > "$bundle/Contents/Resources/使用说明.txt" <<GUIDE
LanSender / LanReceiver — macOS 使用说明（${version}）

本包适用于 ${architecture_description}、macOS ${minimum_os} 及以上版本。
请同时更新 LanSender.app 和 LanReceiver.app；旧版接收端不支持动态容量画面的自动定位。
应用采用本地临时签名，尚未 Apple 公证；无需 Java、WebView 或配置文件。

使用步骤
1. 将两个应用完整解压后运行，也可复制到“应用程序”。
2. 为 LanReceiver 开启“系统设置 → 隐私与安全性 → 屏幕录制”权限，再重新启动接收端。
3. 接收端默认保存到系统桌面，可在设置页选择其他目录。点击“开始接收”，保持发送端画面完整可见，定位成功后点击发送端“发送文件”。
4. 接收端支持连续接收文件；本次接收历史可独立滚动，点击文件名可在 Finder 中定位文件。
5. 发送端窗口靠左，占屏幕工作区约 60% 宽、80% 高；接收端靠右，占约 40% 宽、80% 高。窗口根据可用工作区及显示缩放调整，必要时缩小控件。
6. 两端水印默认开启，水印开关必须一致，传输期间不能修改。发送端水印开关位于发送按钮行中间，接收端位于“开始接收”和“停止接收”之间。深浅主题可在设置页选择。
7. 发送端设置页提供 30 / 50 / 100 毫秒数据帧间隔。30 毫秒速度更快，但接收设备处理不足时更容易丢帧；50 毫秒为默认值，兼顾速度与稳定性；100 毫秒适合性能较弱设备或出现丢帧时使用。选择对所有尺寸的正常数据帧生效，发送或准备文件期间不能更改，重新启动恢复 50 毫秒。
8. 间隔是目标展示时间，实际速度受屏幕刷新、截图和图像解析影响。每次完整发送的校准棋盘格和信息帧各至少展示 1 秒，补发帧至少展示 200 毫秒。
9. 传输画面使用 2 物理像素格子，行列数均为 4 的倍数，至少 160×80 个格子，长宽比为 1.2～3.2。画面尽量利用窗口剩余空间，空间较大时增加格子数量和每帧载荷，减少同一文件的数据帧数量。
10. 发送时保持完整传输画面无遮挡，鼠标移出画面。移动窗口或改变显示尺寸会重新校准，并按新画面容量从头发送。极小窗口放不下完整画面时暂停。如提示缺失帧，可选择更长间隔，并按接收端报告补发。
11. 两端主界面显示当前传输速度，按最近约 1 秒的实际数据量统计；发送端统计已展示的数据，接收端只统计校验通过的新数据，重复帧不计入。等待或停止时归零。接收端等待文件时隐藏蓝色进度，传输时显示加粗的进度条。
12. 等待提示、传输状态及文件名固定在对应行左侧，当前速度固定在右侧；长文本截断显示。
13. 两端图标采用浅米白底、深绿色细线和叠放方框；右上箭头为发送端，左下箭头为接收端。Finder 和 Dock 均使用对应图标。
14. LanSender 的“选择文件”使用与麒麟一致的内置文件选择器，跟随应用深浅主题。支持常用位置、外置磁盘导航、路径输入、当前目录搜索、隐藏文件切换和文件大小及类型查看；输入内容与提示文字垂直居中。单击选择文件，双击进入文件夹或确认文件，底部确认与取消按钮固定，仅文件列表滚动。LanReceiver 设置页仍使用系统目录选择窗口。
15. LanSender 主界面的“全屏 / 退出全屏”按钮或 F11 可切换全屏，Esc 可退出全屏；文件选择器打开时，Esc 先关闭选择器。全屏利用更大的传输画面提高每帧容量，不切换屏幕分辨率，也不缩放编码格子。传输中切换全屏会重新校准并从头发送，建议发送前切换；实际速率受截图、图像解析和显卡性能限制，不保证一定提速。

程序通过屏幕图像传输文件，不使用局域网连接。发送完成表示所有帧已展示，文件是否完整以接收端结果为准。
GUIDE
done
cp target/release/LanSender "$sender_bundle/Contents/MacOS/LanSender"
cp target/release/LanReceiver "$receiver_bundle/Contents/MacOS/LanReceiver"
cp assets/icons/lansender.icns "$sender_bundle/Contents/Resources/lansender.icns"
cp assets/icons/lanreceiver.icns "$receiver_bundle/Contents/Resources/lanreceiver.icns"
cp target/release/LanReceiver dist/LanReceiver
rm -f dist/conf/imgBridge.properties
rmdir dist/conf 2>/dev/null || true
cat > "$sender_bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0//EN">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>LanSender</string>
<key>CFBundleIdentifier</key><string>local.imgbridge.lansender</string>
<key>CFBundleName</key><string>LanSender</string>
<key>CFBundleDisplayName</key><string>LanSender</string>
<key>CFBundleIconFile</key><string>lansender.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>$minimum_os</string>
</dict></plist>
PLIST
cat > "$receiver_bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0//EN">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>LanReceiver</string>
<key>CFBundleIdentifier</key><string>local.imgbridge.lanreceiver</string>
<key>CFBundleName</key><string>LanReceiver</string>
<key>CFBundleDisplayName</key><string>LanReceiver</string>
<key>CFBundleIconFile</key><string>lanreceiver.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>$minimum_os</string>
<key>NSScreenCaptureUsageDescription</key><string>读取发送端屏幕中的传输画面，以接收文件。</string>
</dict></plist>
PLIST
for bundle in "$sender_bundle" "$receiver_bundle"; do
    plutil -lint "$bundle/Contents/Info.plist"
    codesign --force --sign - "$bundle"
    codesign --verify --deep --strict "$bundle"
done
# Stage only the two application bundles, never other dist files.
staging_dir=$(mktemp -d "${TMPDIR:-/tmp}/lanbridge-macos.XXXXXX")
trap 'rm -rf "$staging_dir"' EXIT HUP INT TERM
archive_name=LanBridge-${version}-macos-${archive_arch}
mkdir -p "$staging_dir/$archive_name"
ditto "$sender_bundle" "$staging_dir/$archive_name/LanSender.app"
ditto "$receiver_bundle" "$staging_dir/$archive_name/LanReceiver.app"
ditto -c -k --sequesterRsrc --keepParent "$staging_dir/$archive_name" "dist/$archive_name.zip"
echo "Packaged: $sender_bundle"
echo "Packaged: $receiver_bundle"
echo 'Standalone receiver: dist/LanReceiver'
echo "Application ZIP: dist/$archive_name.zip"
