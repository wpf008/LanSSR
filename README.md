# LanSSR — LanSender / LanReceiver

Rust 实现的屏幕图像文件传输工具。LanSender 将文件编码为屏幕上的棋盘格画面，LanReceiver 截取并解析画面，在不使用局域网连接的情况下传输文件。

项目提供两个独立的桌面应用：**LanSender** 负责发送，**LanReceiver** 负责连续接收。两端保留原来的左右窗口布局：发送端靠左约占工作区 60% 宽、80% 高；接收端靠右约占 40% 宽、80% 高。

## 启动

需要 Rust 1.88 或更新版本。本仓库包含源码、图标、兼容补丁、测试与打包脚本；安装包和构建缓存保留在本地，不随源码提交。下文的 `dist/` 路径是执行打包脚本后生成的本地文件位置。

```bash
cargo build --locked --release --bin LanSender --bin LanReceiver
./target/release/LanSender
./target/release/LanReceiver
```

发送端选择文件后点击“发送文件”。接收端默认将文件保存到系统桌面，点击“开始接收”后会持续等待并接收多份文件；设置页可以更改保存目录和深浅主题。接收历史列表可滚动，点击文件名可在 Finder / 资源管理器中定位文件。

两端默认开启水印，必须使用相同的水印模式。发送端和接收端主界面均显示当前传输速度；接收端等待时不显示蓝色进度点，传输开始后显示加粗进度条。发送端设置页提供 30 / 50 / 100 毫秒数据帧间隔，默认 50 毫秒。程序不读取 `imgBridge.properties`，也不依赖 WebView、Java 或 Visual C++ 运行库。

Windows、macOS 和麒麟的 LanSender 主界面可点击“全屏 / 退出全屏”或按 F11 切换，Esc 退出；文件选择器打开时 Esc 先关闭选择器。建议发送前切换，传输中切换会重新校准并从头发送。全屏扩大每帧容量，不改变屏幕分辨率或编码格子大小；实际速度仍受截图、图像解析和显卡性能限制。

接收端也保留命令行模式：

```bash
./target/release/LanReceiver --cli --output-dir received
./target/release/LanReceiver --list-monitors
./target/release/LanReceiver --monitor 0 --region 100,200,1440,800,4
```

## Windows 7 / 10 / 11 x64

现有本地通用便携包为 `dist/LanBridge-2.0.20-windows-x64.zip`。ZIP 内含 `LanSender.exe` 和 `LanReceiver.exe`，适用于 Windows 7 SP1、Windows 10 和 Windows 11 的 64 位 Intel / AMD 系统。请完整解压后分别启动两个 exe；不需要 WebView2、Java、额外图形 DLL 或另装 Visual C++ 运行库。

LanSender 将待发送文件与“选择文件”缩至操作行约三分之一，后面的三分之二排列发送、水印、停止和重置控件，为传输画面留出更多高度。画面宽高分别利用可用区域，不再限制长宽比；通过纹理分块绘制突破 Windows 7 兼容渲染的 1024 单纹理限制，全屏可按当前分辨率铺满控件下方的区域，保留 2 物理像素格子和最多不足一组格子的边缘余量。每帧容量随画面面积增大，实际速度仍取决于设备处理性能。

LanSender 的发送文件选择弹窗与麒麟版统一，跟随应用深浅主题，提供常用位置和磁盘导航、路径输入、当前目录搜索、隐藏文件切换、文件大小与类型信息；输入框提示及内容垂直居中。LanReceiver 的保存目录选择仍使用 Windows 系统窗口。

界面根据当前图形环境自动选择渲染方式：支持 OpenGL 2.0 及以上时沿用原渲染方式；Windows 7 仅提供系统 OpenGL 1.1 时使用兼容渲染，两个应用可以继续启动。软件渲染速度可能较慢，可在发送端设置页选择 100 毫秒间隔；安装显卡厂商驱动可改善性能。两端窗口位置、水印和文件传输方式保持不变。

该包使用 `x86_64-win7-windows-msvc` 目标和静态 CRT 构建，现代 DPI 接口在运行时检测，Windows 7 使用兼容降级路径。Windows 实机启动与完整收发仍需在目标设备验证。

## macOS

```bash
sh scripts/package-macos.sh
```

现有本地包为 `dist/LanBridge-2.0.19-macos-arm64.zip`，适用于 Apple Silicon、macOS 11.0 及以上版本。打包脚本按当前源码版本生成 `dist/LanSender.app`、`dist/LanReceiver.app` 和对应的双应用 ZIP。首次使用 LanReceiver 前，请在“系统设置 → 隐私与安全性 → 屏幕录制”中授权 **LanReceiver**，然后重启应用。应用使用本地临时签名，尚未 Apple 公证。

LanSender 的发送文件选择弹窗与麒麟版统一，支持常用位置、外置磁盘导航、搜索、隐藏文件和大小及类型信息，并沿用输入框垂直居中与固定底部操作区。LanReceiver 的保存目录选择仍使用 macOS 系统窗口。

## 麒麟桌面 V10 SP1 ARM64

现有本地安装包为 `dist/linux-arm64/LanSSR_2.0.23_arm64.deb`，面向麒麟桌面 V10 SP1、ARM64 / aarch64（飞腾、鲲鹏）。安装后自动创建 **LanSender** 和 **LanReceiver** 两个桌面图标，应用菜单名称与桌面一致。

离线机器在安装包所在目录执行 `sudo dpkg -i LanSSR_2.0.23_arm64.deb`，可直接覆盖升级已有 2.0.x。下载文件名前缀统一为 LanSSR，系统内部包名仍为 `lanbridge` 以兼容升级。Linux 版使用内置文件和目录选择器，无需 `zenity` 或文件选择门户；旧版 2.0.14 因缺少 `zenity` 未完成配置时，也可直接执行该命令覆盖升级，无需强制安装。其他缺少的系统依赖请从对应麒麟版本和 ARM64 架构的安装介质或官方软件源带入；断网时 `apt` 无法下载缺少的包。详细说明见 [麒麟安装说明](packaging/linux/README.zh-CN.md)。

2.0.23 在 Linux 实际使用 X11 / Xwayland 创建窗口时优先使用 EGL，避开部分 ARM 显卡驱动的 GLX 启动错误（`GLXBadContextTag`）；EGL 显示初始化失败时再尝试 GLX。后续配置、上下文或首帧失败不在这一自动回退范围内。Wayland 原生窗口及 Windows、macOS 的策略保持原样；目标显卡上的启动与传输仍需实机验证。

2.0.22 为 UKUI / `ukui-kwin_wayland` 桌面增加原生截屏接口，处理没有 `ZwlrScreencopyManager` 时无法接收的问题；定位发送画面后仅截取该区域。旧版 UKUI 接口通过 PNG 返回每次截屏，实际持续传输速率尚需实机验证，默认 50 毫秒间隔不保证在每台机器上都能跟上。若系统截图通知遮挡棋盘格，请在系统通知设置中关闭截图通知或确保传输区域无遮挡。

## 构建与验证

```bash
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

禁用桌面依赖时可只测试协议、图像、定位和接收状态：

```bash
cargo test --locked --no-default-features
```

架构和协议说明见 [docs/architecture.md](docs/architecture.md)，验证记录见 [docs/validation.md](docs/validation.md)。许可证见 [LICENSE](LICENSE)。
