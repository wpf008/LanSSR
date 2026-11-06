# 验证记录

2.0.23 麒麟 V10 SP1 ARM64 / X11 EGL 启动兼容（2026-10-20）：

| 项目 | 结果 |
| --- | --- |
| 问题与修复 | 用户在内核 `5.4.18-53-generic` 的麒麟机器上启动时报 `GLXBadContextTag`，错误在 winit 的 `XMapRaised` 请求刷新时显现，随后锁中毒与析构 panic 导致退出。新增 glutin-winit 0.5.0 本地补丁，在 Linux 实际窗口 display handle 为 Xlib / Xcb 时优先初始化 EGL，避开先走 GLX 的路径；不依据会话环境变量猜测窗口后端 |
| 选择与回退范围 | `LANSSR_GL_BACKEND=auto` 为默认，仅在 EGL display 初始化失败时回退 GLX；后续配置、上下文、表面或首帧失败不自动重试。`egl` / `glx` 严格指定后端，未知值提示后按 auto 处理，实际选中 API 输出到 stderr。原生 Wayland、Windows 与 macOS 沿用原策略；不强制软件渲染，不修改系统驱动或会话设置 |
| 图形接口回归 | 6 项独立 probe 场景全部通过，调用补丁后的真实 glutin-winit，并使用私有 Xvfb / Mesa 创建 EGL 1.5 或 GLX 1.4 上下文。验证 `Wait(1)` 交换间隔、清屏与像素读取 `[51,115,204,255]`、缓冲交换和 XSync 后窗口映射。通过注入 GLX 调用陷阱及 EGL 初始化失败覆盖默认 EGL、显式 EGL、严格 GLX 正控、auto 回退 GLX、严格 EGL 失败及未知值；严格 GLX 正控以退出码 86 证明陷阱生效，严格 EGL 不可用按普通错误退出且没有 panic。这是故障注入，未复现实体驱动的原始 GLX 错误 |
| 实际应用窗口 | 4 项 release 应用检查全部通过：GLX 调用陷阱启用时，默认 LanSender / LanReceiver 均选 EGL、窗口映射并存活 1 秒；注入 EGL 初始化失败时，两应用均自动选 GLX、窗口映射并存活 1 秒。窗口尺寸分别为 1152×800、768×800；这些检查不执行文件传输 |
| 构建与测试 | Debian 11 ARM64 / glibc 2.31 / Rust 1.88.0 的全目标 check、release 构建及 125 项项目测试通过（119 库 + 6 图像流程，1 项 Java 互通 ignored）。宿主 macOS 全目标 Clippy `-D warnings` 通过；本地 rfd 的 42 项既有警告与 xcap 的 3 项 macOS 上游弃用警告保留。构建器未安装 Clippy，未联网安装组件；本轮未重跑 2.0.22 的旧 Wayland 协议和 UKUI 原生截屏 fixture |
| 包与离线安装 | 两个 ELF 均为 AArch64，最高 GLIBC 符号版本为 2.29；包内二进制与 release 逐字节一致，新增 glutin-winit 的完整 MIT 许可证逐字核验通过。断网首次安装及 2.0.22 → 2.0.23 直接升级通过，`dpkg --audit`、动态库、命令入口、版本、帮助与其他工作目录下的内置默认参数检查通过；无 zenity 强依赖 |
| 桌面与已安装 GUI | 首次安装及覆盖升级的桌面图标、重复安装与卸载检查通过，覆盖中文 XDG 路径、精确应用名、权限与用户所有权、保留自定义文件。最终安装包的两应用在 Xvfb / Mesa 中均选 EGL、窗口映射可见并存活 2 秒；未在宿主机安装 deb |
| 验证范围 | 本轮仅重新打包麒麟 ARM64，Windows 与 macOS 现有发布包保持原样。模拟环境与故障注入不能替代目标麒麟显卡、驱动和内核的实机验证；尚未验证本版实机启动、全屏、完整文件传输或持续速率。此前的 UKUI 原生截屏补丁保留，但本轮不将既有结果表述为重新验证 |

产物为 `dist/linux-arm64/LanSSR_2.0.23_arm64.deb`（8,657,088 字节），SHA256 为 `8f72b92b0c2e7c513742d511cf7c6482de0f03300c3e84f640e16a86aa50ab60`，记录于 `dist/SHA256SUMS`。安装命令：`sudo dpkg -i LanSSR_2.0.23_arm64.deb`。日志位于 `dist/validation-2.0.23/`，包括 `kylin-build-tests.log`、`macos-clippy.log`、`linux-graphics-backend.log`、`kylin-package.log`、`kylin-clean-install-gui.log`、`kylin-gui-smoke.log` 和 `kylin-upgrade.log`。图形回归工具保留在 `scripts/validation-linux-graphics/`；补丁来源与边界见 `vendor/glutin-winit/LANSSR-PATCH.md`。

2.0.22 麒麟 V10 SP1 ARM64 / UKUI 原生 Wayland 截屏兼容（2026-10-13）：

| 项目 | 结果 |
| --- | --- |
| 问题与修复 | 用户实机为 `XDG_SESSION_TYPE=wayland`、`XDG_CURRENT_DESKTOP=UKUI`，运行 `ukui-kwin_wayland` 与 Xwayland。2.0.21 已能启动，但接收时因桌面不提供 `ZwlrScreencopyManager` 报错。新增旧版 UKUI / KWin 的原生 D-Bus 截屏路径，优先探测 `org.ukui.KWin`、兼容别名 `org.ukui.kwin` 及 `org.kde.KWin`，调用非交互的 `screenshotArea(i,i,i,i,b) -> s` 并读取返回的 PNG；现有 GNOME、门户及 wlroots 后端继续保留 |
| 连续截屏与区域 | 接收线程复用已成功的截屏后端和 D-Bus 连接，定位后使用实际图像坐标只截取传输区域；UKUI / KWin 区域请求计入显示器原点。返回空路径、失效文件、坏 PNG 或尺寸不符时返回错误，不复用上一帧，按条件清理临时截图。显式 X11 会话忽略残留的 `WAYLAND_DISPLAY`；真正 Wayland 不降级为 Xwayland 根窗口截屏 |
| 原生接口回归 | 9 项 fixture 全部通过：第二进程在私有 `dbus-daemon` 上注册实际 UKUI / KDE 服务名、对象、接口及方法，客户端使用生产 xcap 截屏代码。覆盖 UKUI 主名与别名、KDE、两服务共存、UKUI 拒绝后回退并缓存 KDE、旧 Wayland 缺少 screencopy、无截屏服务及显式 X11。逐像素验证完整截图与已选后端的 `capture_image_region` 区域截图、帧更新、失败后恢复和临时 PNG 清理；X11 场景使用真实 XCB 且不调用 D-Bus 截图 |
| 实际接收循环 | 上述第 9 项调用生产 `receiver::run_continuous`，在 1280×720 的 fixture 桌面 PNG 中自动定位位于 `(112,84)` 的 512×320 水印棋盘，使用原生区域截屏连续接收两个文件。共保存 47,214 字节，逐字节比较通过，未重复保存；桌面图像随接收事件切换校准、元数据及数据帧。该结果验证实际接收代码与 D-Bus / PNG 路径，不是实体麒麟桌面的完整传输测试 |
| 旧协议回归 | 45 项真实 Wayland socket 回归全部通过，继续覆盖旧版 wl_output、xdg-output 与 screencopy 的协议组合、像素、重复截屏、缺少协议及错误处理；每项运行实际 xcap 显示器枚举与启动几何路径，保持 2.0.21 的旧协议启动兼容 |
| 构建与测试 | Debian 11 ARM64 / glibc 2.31 / Rust 1.88.0 的全目标 `cargo check`、release 构建和 125 项 Linux 项目测试通过（119 库 + 6 图像流程，1 项 Java 互通 ignored）。宿主 macOS 全目标 Clippy `-D warnings` 与 Rust 格式检查通过；本地 rfd 的 42 项上游警告和 xcap 的 3 项 macOS 上游弃用警告保留。ARM64 构建器未安装 Clippy，未联网安装组件；构建、协议、D-Bus、打包与安装验证均使用断网容器 |
| 包与离线安装 | 两个 ELF 均为 AArch64，最高 GLIBC 符号 2.29；包内二进制与 release 逐字节一致，新增 xcap 的完整 Apache 2.0 许可证核验通过。断网首次安装与 2.0.21 → 2.0.22 直接升级通过，`dpkg --audit`、动态库、命令入口、版本、帮助及其他工作目录下的内置默认参数检查通过；无 zenity 强依赖 |
| 桌面与 GUI | 首次安装、覆盖升级的桌面图标与卸载检查通过，包括中文 XDG 路径、精确应用名、权限及用户所有权、重复安装和保留用户自定义文件。Xvfb + Mesa 中 LanSender 的 1152×800 窗口与 LanReceiver 的 768×800 窗口均映射可见并存活 2 秒；仅检查已安装应用的窗口启动 |
| 验证范围与限制 | 本轮仅重新打包麒麟 ARM64；Windows 与 macOS 现有包保持原样。D-Bus fixture 模拟截图服务，Wayland fixture 不是完整 UKUI 合成器，Xvfb 也不是用户实体桌面。未在目标实机验证完整收发、持续速率、截图通知或实际桌面缩放。旧 UKUI 每次压缩、落盘及读取 PNG，不能保证 50 毫秒间隔；截图通知可能遮挡棋盘，逻辑尺寸 PNG 在较高缩放下的水印像素也需实测。应用不更改系统通知或会话设置 |

产物为 `dist/linux-arm64/LanSSR_2.0.22_arm64.deb`（8,657,318 字节），SHA256 为 `74d6908d5aa39f2dc442ced2afb28d25759274c07c49ec696978cac329e47782`，记录于 `dist/SHA256SUMS`。安装命令：`sudo dpkg -i LanSSR_2.0.22_arm64.deb`。日志位于 `dist/validation-2.0.22/`，包括 `kylin-tests.log`、`wayland-legacy-protocol.log`、`native-dbus-capture.log`、`macos-clippy.log`、`kylin-build-package.log`、`kylin-clean-install-gui.log`、`kylin-gui-smoke.log` 和 `kylin-upgrade.log`；包与环境汇总为 `kylin-package-report.json`。原生接口及接收循环验证工具保留在 `scripts/validation-linux-capture/`，旧协议回归工具保留在 `scripts/validation-wayland/`。若旧 UKUI 截图速度跟不上，可先将发送端间隔改为 100 毫秒；系统截图通知遮挡传输画面时，可在系统通知设置中关闭相应通知或调整窗口位置。

2.0.21 麒麟 V10 SP1 ARM64 旧版 Wayland 启动兼容（2026-10-13）：

| 项目 | 结果 |
| --- | --- |
| 问题与修复 | 用户在内核 `5.4.96-27-kr9a0` 的麒麟桌面启动两个应用时报 `libwayshot-xcap` 的 `UnsupportedVersion`。依赖在启动窗口尺寸估算中强制绑定 xdg-output v3，遇到旧版协议直接 panic。本地补丁协商 xdg-output v1–v3、wl_output v1–v4，并通过 Result 返回缺失协议；验证输出尺寸后计算缩放 |
| 接收兼容 | SHM screencopy 协商 v1–v3：v1/v2 在 Buffer 后复制，v3 等待 BufferDone；格式阶段和复制阶段的 Failed 均返回错误。支持 y_invert 行翻转，销毁完成或拒绝的 SHM 帧。DMABUF 保持 v3 要求，未强制使用 Xwayland 根窗口截屏，也未修改会话环境变量 |
| 协议回归 | 45 项真实 Wayland socket 测试全部通过，包含 wl_output v1/v2/v4、xdg-output v1/v2/v3、screencopy v1/v2/v3 的合法组合，逐个检查 512 个 RGBA 像素、名称、坐标、重复截屏与刷新；缺少协议、无显示器、提前 Failed、复制 Failed、未知格式和上下翻转均按预期处理且无挂起 |
| 启动路径回归 | 上述每个测试均调用实际 `xcap::Monitor::all`、`is_primary`、`scale_factor`、`x/y/width/height`，与两个应用的窗口估算路径一致。私有 Xvfb 提供真实 XCB/RandR 数据，旧 Wayland 服务提供缩放信息；正常为 scale 2，缺失输出信息走 scale 1 回退，没有启动 panic |
| 构建与测试 | Debian 11 ARM64 / glibc 2.31 / Rust 1.88.0，release 构建和 125 项 Linux 项目测试通过（119 库 + 6 图像流程，1 项 Java 互通 ignored）。构建器未安装 Clippy，未联网安装组件；宿主 macOS 的 Clippy 全目标检查及 Rust 格式检查通过。构建、协议、打包与安装验证均使用断网 ARM64 容器 |
| 包与安装 | 两个 ELF 均为 AArch64，最高 GLIBC 符号 2.29；包内二进制与 release 逐字节一致，包含新增 BSD 许可证。断网首次安装、2.0.20 → 2.0.21 覆盖升级、动态库、命令入口、版本、桌面图标和卸载检查通过；无 zenity 强依赖 |
| GUI 与范围 | Xvfb + Mesa 中 LanSender、LanReceiver 窗口均映射并存活 2 秒。旧协议 fixture 不是完整桌面合成器，本轮未在用户实体麒麟上验证 GUI 集成、桌面授权、完整屏幕文件收发或速率。仅重新打包麒麟 ARM64 |

产物为 `dist/linux-arm64/LanSSR_2.0.21_arm64.deb`（8,638,932 字节），SHA256 为 `758781a882ab4b61a1937bbdfc978b7b9ccbff8816bc2362a5ef453841175089`，已记录于 `dist/SHA256SUMS`。安装命令：`sudo dpkg -i LanSSR_2.0.21_arm64.deb`。日志位于 `dist/validation-2.0.21/`，包括 `kylin-build.log`、`kylin-package.log`、`wayland-legacy-protocol.log`、`kylin-clean-install-gui.log`、`kylin-gui-smoke.log`、`kylin-upgrade.log` 和 `macos-clippy.log`。协议测试工具与独立锁文件保留在 `scripts/validation-wayland/`。首次桌面升级检查错误地用图形安装场景准备旧版、再断言其他用户没有图标，造成测试场景冲突；修正为同一 sudo 用户安装旧版后完整通过，初始日志保留为 `kylin-upgrade-initial.log`。

2.0.20 麒麟桌面 V10 SP1 ARM64 重新打包（2026-10-13）：

| 项目 | 结果 |
| --- | --- |
| 发布范围 | 重新生成 `LanSSR_2.0.20_arm64.deb`，包含独立的 LanSender 和 LanReceiver；内部包名仍为 `lanbridge`，可覆盖升级旧版。安装说明和下载前缀同步为 2.0.20 |
| 构建与目标 | 使用缓存 Debian 11 ARM64 / glibc 2.31 / Rust 1.88.0 构建器；构建、测试、打包和安装检查均在 `--network none` 容器中完成 |
| 自动测试 | Linux 119 项库测试与 6 项图像流程集成测试通过，共 125 项；既有 Java 互通测试 1 项 ignored。覆盖分块纹理、全屏切换、超宽/竖屏定位、文件往返和水印像素 |
| 包审计 | 两个 ELF 均为 AArch64，最高 GLIBC 符号版本 2.29；包内二进制与 release 文件逐字节一致，桌面入口、图标、许可证和内置设置通过。`Depends` 不包含 `zenity`，桌面门户仅为可选建议 |
| 断网安装 | 首次安装、LanSender/LanReceiver `--help` / `--version`、动态库检查和从其他工作目录启动内置默认参数均通过；2.0.15 → 2.0.20 覆盖升级通过 |
| 桌面图标与 GUI | 首次安装及升级场景的 LanSender / LanReceiver 桌面图标完整检查通过；Xvfb + Mesa 中两个窗口均映射并存活 2 秒。未做实体麒麟机器的全屏几何、完整屏幕收发或实测速率验证 |

产物为 `dist/linux-arm64/LanSSR_2.0.20_arm64.deb`（8,636,412 字节），SHA256 为 `08178cd86aa9fc915bc66295f76fe27ae146f0331b6c9dca5d12236b4b66357b`，已记录于 `dist/SHA256SUMS`。日志位于 `dist/validation-2.0.20/` 的 `kylin-build.log`、`kylin-clean-install.log`、`kylin-upgrade.log` 和 `kylin-gui-container.log`。本版安装命令为 `sudo dpkg -i LanSSR_2.0.20_arm64.deb`。

2.0.20 Windows 全屏传输画面铺满与发送控件布局（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 问题与改动 | Windows 全屏时传输画面仍受 1024 单纹理上限及长宽比约束，左右留出大块空白。传输画面现按剩余可用宽高分别扩展，取消长宽比约束，并按渲染器支持的纹理尺寸分块绘制；不拉伸编码格子，保留 2 物理像素格子与 8 物理像素的宽高对齐。较大画面提高每帧容量，未将容量变化表述为实际吞吐量提升 |
| 操作行布局 | 待发送文件和“选择文件”占操作行约三分之一；“发送文件”、水印开关、“停止”和“重置”位于后面的三分之二，同排显示，释放原按钮行的高度。保留标题、设置及全屏入口、补发与传输状态；小窗口按可用空间调整控件 |
| 自动检查 | macOS 宿主执行 117 项库测试与 6 项图像流程集成测试通过，共 123 项；需要 Java 的既有互通测试 1 项为 ignored，本轮未执行。LanSender 定向 28 项、纹理分块 4 项、定位器 14 项及图像往返流程 6 项均通过；这些定向检查与全目标测试存在重叠，不累加为独立测试数量 |
| 静态与格式检查 | 全目标 Clippy `-D warnings` 和 Rust 格式检查通过；本地 rfd 保留 42 项既有上游警告。Windows 打包脚本 Shell 语法、内嵌 Python 语法、清单与锁文件版本一致性、各平台发布链接检查通过 |
| 全屏与布局测试 | egui 输入和几何模拟覆盖 1920×1080、3840×2160、3440×1440、竖屏及 920×1080、深浅主题和不同 DPI；模拟 Windows 7 的 1024 纹理上限。传输画面左右合计及底部剩余均不足 8 物理像素，文件选择区域约占三分之一，其余控件位于同一操作行；进入与退出全屏期间暂停、重新校准并从头发送的检查通过 |
| 纹理与协议测试 | 完整编码画面分块上传，单块纹理不超过渲染器上限；自动检查覆盖全部像素恢复、跨块格子与水印解码、邻块边界及不同 DPI 下物理像素对齐。定位和往返测试通过；不改变编码格子与传输协议 |
| 实际渲染与读取 | 断网 ARM64 容器内通过 Xvfb + Mesa llvmpipe，直接复用生产 `TransferTexture` 源码，分别运行 OpenGL 1.1 固定管线及 OpenGL 2.1 着色器渲染。应用层设定单块上限 1024，1936×1088 画面分为 4 块；水印开关两种模式 × 4 种 DPI × 2 个变化帧 × 2 条渲染路径，共 32 帧读取结果逐像素与原图相等且协议解码通过。纹理句柄复用、空闲不上传检查通过。Mesa 驱动实际上限为 16384，本测试不是 Windows 7 或真实 1024 上限硬件验证 |
| 原生布局观察 | Linux ARM64 原生 LanSender 在 Xvfb 中显示 1152×800 主窗口，实际查看 `linux-layout-qa/00-main.png`，新的文件选择与操作控件同排且未裁剪。该截图仅验证非全屏主界面布局，不能替代 Windows 实机全屏几何或传输验证 |
| Windows 分发 | 使用缓存的 cargo-xwin、匹配 Rust 1.98.1 标准库源码、`x86_64-win7-windows-msvc` 及静态 CRT 离线交叉编译；两个程序的 PE OS / GUI 子系统版本均为 6.1。Windows 7 普通及 delay 静态导入、六文件 ZIP 白名单与 CRC、release / 包内二进制及许可证一致性检查通过。保留 OpenGL 1.1 兼容路径，无新增外部图形 DLL、WebView2 或动态 CRT 依赖 |
| 发布范围与限制 | 本轮生成 Windows 7 SP1 / 10 / 11 x64 通用包；macOS 保留 2.0.19，麒麟 ARM64 已同步为 2.0.20。未进行 Windows 实机 GUI、完整屏幕文件收发或传输速率测试；静态导入与宿主自动测试不能替代目标系统的实际运行验证 |

产物为 `dist/LanBridge-2.0.20-windows-x64.zip`（5,507,018 字节），SHA256 为 `6c0fc699fb2530a9987b268b7e985f7aed80c8eb2c1b82b943ffd28c13f97831`，记录于 `dist/SHA256SUMS`。全目标测试、Sender 定向、Clippy 与 Windows 构建日志分别为 `dist/validation-2.0.20/` 的 `tests.log`、`sender-tests.log`、`clippy.log` 和 `windows-build.log`，Windows 包审计报告为 `windows-build-report.txt`。实际分块渲染日志为同目录的 `tiled-gpu-opengl-1.1.log` 和 `tiled-gpu-opengl-2.1.log`；原生非全屏布局截图、几何记录与运行日志位于 `linux-layout-qa/`。原生布局和 GPU 检查使用一次性断网 ARM64 容器，未生成或覆盖现有麒麟安装包，也未向宿主系统安装 deb。

2.0.19 麒麟桌面 V10 SP1 ARM64 全屏版打包（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 发布范围 | 本轮生成 `LanSSR_2.0.19_arm64.deb`，包含独立的 LanSender 和 LanReceiver；内部包名仍为 `lanbridge`。Windows 与 macOS 的现有 2.0.19 包保持原样 |
| 全屏功能 | 麒麟版使用同一 LanSender 全屏实现，支持主界面“全屏 / 退出全屏”按钮、F11 切换和 Esc 退出，文件选择器打开时 Esc 先关闭选择器。全屏扩大每帧容量，传输中切换会重新校准并从头发送，建议发送前开启；未将每帧容量变化表述为实测吞吐量提升 |
| 构建与目标 | 使用缓存的 Debian 11 ARM64 / glibc 2.31 / Rust 1.88.0 构建器，构建、测试、打包与容器安装验证均使用 `--network none`，没有下载依赖或在 macOS 宿主机安装 deb |
| Linux 自动测试 | 112 项库测试与 5 项图像流程集成测试通过，共 117 项；包含 4 项全屏定向测试，覆盖画面容量、切换暂停与重新校准、退出恢复及选择器优先处理 Esc。需要 Java 的既有互通测试 1 项为 ignored，本轮未执行；Linux Clippy 未执行 |
| ELF 与包内容 | 两个程序均为 AArch64，最高所需 GLIBC 符号版本为 2.29；包内程序与本轮 release 文件逐字节一致，菜单、图标、许可证和内置设置检查通过。包内安装说明与冻结的源文件字节一致；`Depends` 不包含 `zenity`，桌面门户仅为可选建议 |
| 断网安装与升级 | 在两个独立 ARM64 容器中分别完成首次安装与已配置的 2.0.15 → 2.0.19 直接升级；未安装 `zenity` 或桌面门户，未使用强制安装选项。`dpkg --audit`、无缺失动态库、两程序 `--help` / `--version` 和从其他工作目录使用内置默认参数启动的检查通过。本轮未重复 2.0.16 → 2.0.19 的安装升级流程 |
| 桌面快捷方式 | 本轮在首次安装与 2.0.15 升级场景分别运行完整快捷方式验证，中文 XDG 桌面路径（含空格）、精确应用名 LanSender / LanReceiver、命令与图标、0755 权限及用户所有权通过；重复安装、图形安装器普通账号回退、禁用桌面目录、保留用户自定义文件和符号链接、保留修改及卸载清理均通过。XDG 配置未执行用户代码；用户身份的 trusted 元数据后端失败不阻塞安装 |
| 原生窗口烟测 | 新包在另一断网容器安装后，通过 Xvfb + Mesa 软件 OpenGL 检查；LanSender 的 1152×800 窗口与 LanReceiver 的 768×800 窗口均映射可见并持续运行 2 秒，仅停止脚本自有进程。此结果仅证明窗口启动与短时存活 |
| 验证限制 | 缓存构建器没有 EWMH 窗口管理器；Xvfb 中的 F11 / Esc 输入观察不构成真实全屏尺寸验收。未进行麒麟桌面 V10 SP1 实机的全屏几何、显卡兼容、完整屏幕文件传输或传输速率验证；全屏状态与画面容量依据上述自动测试 |

产物为 `dist/linux-arm64/LanSSR_2.0.19_arm64.deb`（8,632,692 字节），SHA256 为 `c60788ecfe42d259bd62b1c2dc9a15a55c9091ec6280f432c05423fb5705c81d`，记录于 `dist/SHA256SUMS`。本轮日志位于 `dist/validation-2.0.19/` 的 `kylin-build.log`、`kylin-clean-install.log`、`kylin-upgrade.log`、`kylin-gui-smoke.log` 和 `kylin-gui-container.log`，环境及包核验汇总为同目录的 `kylin-package-report.json`；无窗口管理器的原始输入观察保存在 `kylin-gui-fullscreen/`，不作为全屏几何验证结果。离线安装命令为 `sudo dpkg -i LanSSR_2.0.19_arm64.deb`，安装后保留两个独立应用入口和桌面图标。

2.0.19 LanSender 全屏传输界面（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 操作方式 | Windows 和 macOS 的 LanSender 主界面增加“全屏 / 退出全屏”按钮，F11 切换，Esc 退出；文件选择器打开时 Esc 先关闭选择器 |
| 传输画面 | 全屏利用更大的可用空间增加每帧容量，不改变系统屏幕分辨率，也不缩放编码格子；传输中切换会重新校准并从头发送，建议发送前切换。实际传输速率受截图、图像解析与显卡性能限制，不保证一定提速 |
| 自动检查 | 110 项库测试与 5 项图像流程集成测试通过，共 115 项；需要 Java 的既有互通测试 1 项为 ignored，本轮未执行。新增 4 项全屏定向测试另行通过；Rust 格式检查与 all-targets Clippy `-D warnings` 通过，本地 rfd 保留 42 项既有上游警告。代码独立只读审查通过 |
| 全屏布局测试 | 自动测试覆盖深浅主题、1366×768 / 1920×1080 / 3840×2160 及竖屏、DPI 1 / 1.25 / 2 和负坐标屏幕；验证编码格子保持 2 物理像素、画面尺寸保持 8 像素倍数、每帧载荷随可用空间增加。进入全屏后不再发送强制窗口尺寸或位置指令，交由系统管理全屏几何 |
| 切换与传输测试 | 进入及退出全屏期间暂停，等待原生全屏状态、窗口几何及 DPI 稳定后重新校准；覆盖原生状态先变而几何后变、DPI 变化再次等待的情况。500,000 字节缓存文件重新分片与校准、退出后恢复窗口布局测试通过；Windows 7 纹理上限 1024 的水印画面解码、选择器优先处理 Esc 测试通过。这些结果为自动输入与状态模拟，未作为目标系统实机传输速率证据 |
| Windows 分发 | 通用包的 PE、Windows 7 普通及 delay 静态导入、ZIP 白名单及 CRC、release / 包内二进制和许可证一致性审计通过；保留 Windows 7 兼容构建及渲染方式，无新增外部 DLL |
| macOS 分发 | 两个完整应用及解压后的 ARM64 架构、最低 macOS 11.0、系统动态库、版本、`--help` / `--version`、本地签名、可执行权限、图标和许可证检查通过。归一化签名长度相关字段后包内 Mach-O 的编译代码与数据和 release 文件一致；ZIP 中文文件名、两应用白名单、CRC 及解压后逐文件一致性通过。应用尚未 Apple 公证 |
| macOS 原生全屏检查 | 使用独立临时应用副本，实际点击“全屏”、按 Esc 退出，再按 F11 进入及退出，窗口布局正常恢复；副本仅修改应用元数据、可执行文件名及本地签名，去除签名后的程序代码与正式分发应用一致。实际查看窗口及全屏截图，控件和完整传输画面显示正常；窗口模式画面为 1744×616、显示每帧容量 131.1 KB，全屏为 2952×1208、435.3 KB，每帧容量约增加至 3.3 倍。这是画面容量变化，本轮未发送文件或测量实际吞吐量，不作为传输速率提高 3.3 倍的结论。两次原生进入 / 退出全屏后应用保持响应，本轮 CUA 输入无错误；所有测试副本进程已退出，未操作用户已有应用或修改正式发布包。应用窗口截图不独立证明全局桌面坐标 |
| 发布范围 | Windows 7 SP1 / 10 / 11 x64 通用包和 macOS ARM64 双应用包更新为 2.0.19；麒麟包保留 `LanSSR_2.0.16_arm64.deb` |

Windows 通用包为 `dist/LanBridge-2.0.19-windows-x64.zip`（5,504,915 字节），SHA256 为 `63ab2ede5eb1d6afc64f0821049b4b6b36b9b8802e7f3976b4c36eecc27fffa9`；macOS 双应用包为 `dist/LanBridge-2.0.19-macos-arm64.zip`（5,367,211 字节），SHA256 为 `6da4454d53613015b9b6fabe85121abca4fb5dd429be00bf7db048f1c0a0bd50`。两包校验值记录于 `dist/SHA256SUMS`。全屏定向测试、全目标测试、Clippy 与两个平台构建日志位于 `dist/validation-2.0.19/` 的 `fullscreen-tests.log`、`tests.log`、`clippy.log`、`windows-build.log` 和 `macos-build.log`；Windows 包报告为同目录的 `windows-build-report.txt`，macOS 包报告及复现脚本为 `macos-package-report.txt` 和 `verify-macos-package.py`。同目录的 `macos-fullscreen/` 保存原生 QA 的 `gui-report.txt` / `gui-report.json`、副本一致性记录 `clone-verification.json`，以及按实际画面内容确认的 `01-windowed.jpg`（1814×1374）和 `02-fullscreen.jpg`（3024×1898）；其余观察截图不单独对应某个快捷键的成功结果。本轮未进行 Windows 7 / 10 / 11 实机 GUI 或完整文件传输速率验证。

2.0.18 Windows 接收保存路径显示修复（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 用户路径显示 | LanReceiver 的保存目录、接收历史完整路径与悬浮提示、相关文件管理器错误以及接收 CLI 提示统一使用 Windows 路径展示处理。普通盘符路径去除系统规范化产生的 `\\?\` 前缀，UNC 路径恢复为 `\\server\share` 形式；卷 GUID 和设备路径保留原样 |
| 保存与定位 | 展示处理仅用于用户可见文字，实际规范化后的 `PathBuf`、文件保存、目录打开及历史文件定位继续使用原路径；不改变接收默认桌面目录或传输协议 |
| 定向检查 | 4 项 `desktop_paths` 定向测试通过，覆盖盘符、UNC 与特殊路径展示、桌面配置解析和文件管理器操作的前置检查；本轮未重跑全部库及集成测试。Rust 格式检查和 all-targets Clippy `-D warnings` 通过，本地 rfd 保留 42 项既有上游警告 |
| Windows 分发 | Windows 7 SP1 / 10 / 11 x64 通用包构建成功，自动 ZIP 白名单及 CRC、release / 包内二进制及许可证一致性、PE 和 Windows 7 普通及 delay 导入审计通过 |
| 发布范围 | 本轮仅重新生成 Windows 7 SP1 / 10 / 11 x64 通用包；macOS 发布包保留 2.0.17，麒麟包保留 `LanSSR_2.0.16_arm64.deb` |

产物为 `dist/LanBridge-2.0.18-windows-x64.zip`（5,502,717 字节），SHA256 为 `6401175cbb76a979e6b932e4dd874ff21da87fce167cf27afe4a77ffde64a1a5`，记录于 `dist/SHA256SUMS`。构建、定向测试、Clippy 日志与包审计报告位于 `dist/validation-2.0.18/` 的 `windows-build.log`、`path-tests.log`、`clippy.log` 和 `windows-build-report.txt`。本轮未进行 Windows 实机 GUI 或完整传输验证；接收核心保存异常的第三方及原有路径文案未在本次修复范围内，不宣称所有错误信息均去除了系统路径前缀。

2.0.17 Windows / macOS 发送文件选择界面统一（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 变更范围 | Windows 和 macOS 的 LanSender 改用与麒麟版共用的内置文件选择器；LanReceiver 在这两个平台仍使用系统保存目录选择窗口。两个独立应用、左右窗口位置、协议和传输时序保持原有逻辑；本轮不重新生成麒麟 deb，现有 `LanSSR_2.0.16_arm64.deb` 保留 |
| 文件选择界面 | 沿用麒麟版的深浅主题、常用位置、路径输入、当前目录搜索、隐藏文件切换、文件大小与类型信息；路径和搜索输入内容及提示文字垂直居中。支持单击选择、双击进入文件夹或确认文件、刷新及取消；底部选择信息与操作按钮固定，文件列表使用虚拟滚动 |
| 平台路径适配 | Windows 优先读取 `USERPROFILE`，通过 Windows 7 已有的 `GetLogicalDrives` 获取磁盘导航，不探测每个网络盘；支持盘符、UNC、当前盘根目录、相对路径及 `~\`，并识别系统隐藏文件属性。macOS 增加 `/Volumes` 磁盘入口。共享路径和盘符生成测试在 macOS 上运行通过；仅 Windows 编译的根目录、UNC 和 `~\` 专用测试本轮未在 Windows 实机运行 |
| 自动测试 | 105 项库测试和 5 项图像流程集成测试通过，共 110 项；包含共享选择器和 LanSender 的按钮、取消与确认流程，覆盖中文及空格路径、目录导航、搜索、隐藏文件、小窗口缩放和深浅主题。需要 Java 的既有互通测试 1 项为 ignored，本轮未执行 |
| 静态与格式检查 | Rust 格式检查及项目 Clippy `-D warnings` 通过，本地 rfd 保留 42 项既有上游警告；两个打包脚本的 Shell 语法检查通过。rfd 的 `ole32!CoTaskMemFree` 兼容补丁保留，用于接收端系统目录选择 |
| Windows 分发 | 继续使用 `x86_64-win7-windows-msvc`、匹配 Rust 1.98.1 标准库源码和静态 CRT，两个 exe 的 PE OS / GUI 子系统版本均为 6.1；Windows 7 基线普通及 delay 导入审计通过。六文件白名单、ZIP CRC、release / 包内二进制及许可证一致性通过；兼容 OpenGL 1.1 的渲染路径保留，无新增 WebView2、动态 CRT 或外部图形 DLL |
| macOS 分发 | 两个独立应用均为 ARM64、最低 macOS 11.0，版本与 `--help` / `--version`、可执行权限、系统动态库、许可证和对应 ICNS 图标检查通过；完整应用及 ZIP 解压后本地签名通过，尚未 Apple 公证。ZIP 中文文件名、精确两应用白名单、CRC 和解压后逐文件一致性通过；仅归一化签名长度相关字段后，包内 Mach-O 的编译代码及数据与 release 文件一致 |
| macOS 原生界面检查 | 在独立临时 `.app` 副本中打开 LanSender 文件选择器，仅修改副本的应用元数据及本地签名，去除签名后的可执行文件与分发应用逐字节一致；未修改正式发布包，也未操作用户已有窗口。实际查看 `01-picker-light.png`，浅色布局与麒麟共享界面一致，常用位置、搜索、中文及空格文件名、类型和大小、固定确认 / 取消区均显示正常，路径和搜索输入框的内容及提示文字垂直居中 |
| GUI 验证范围 | macOS 原生界面的搜索、确认、取消及深色主题操作未完成：CUA 点击返回 `noWindowsAvailable`，重新绑定与窗口 Raise 后仍受工具窗口定位限制；截图和辅助功能窗口观察正常，测试进程持续存活，没有应用崩溃证据。上述操作及深浅主题已有自动 egui 输入测试通过，不能替代原生 GUI 的完整交互验证；本轮未进行真实屏幕收发。测试副本进程已退出，未触碰用户现有窗口 |
| 打包说明 | Windows ZIP 及两个 macOS 应用的使用说明均增加本版 Sender 内置文件选择器功能与接收端系统目录选择说明；README 的 Windows / macOS 下载链接更新为 2.0.17，Linux 下载链接保留 2.0.16 |

Windows 通用包为 `dist/LanBridge-2.0.17-windows-x64.zip`（5,503,695 字节），SHA256 为 `a2f06f6f84de6cba1ea3828a0c0274fb9ad698d1e716253cb52d0743eacd4954`；macOS 双应用包为 `dist/LanBridge-2.0.17-macos-arm64.zip`（5,364,058 字节），SHA256 为 `d34a6203b7ef6ee1bea701c05124b8acd0a733ba1456ab17e651dd077c0bb130`。Windows 静态分发检查报告位于 `dist/validation-2.0.17/windows-build-report.txt`；macOS 包检查报告与复现脚本分别为同目录的 `macos-package-report.txt` 和 `verify-macos-package.py`。同目录的 `macos-picker/` 中保存原生浅色选择器截图 `01-picker-light.png`、GUI 结果 `gui-report.json` / `gui-report.txt` 与临时副本一致性记录 `clone-verification.json`。静态导入审计不等于 Windows 实机 GUI 或完整文件传输验证；本轮未验证 Windows 7 / 10 / 11 实机收发与传输速率。

2.0.16 麒麟桌面 V10 SP1 ARM64 桌面图标与文件选择界面（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 命名与升级 | 下载文件名前缀统一为 `LanSSR`，产物为 `LanSSR_2.0.16_arm64.deb`。系统内部 `Package: lanbridge`、安装路径和两个命令入口保持兼容，现有正常安装的 2.0.15 可直接覆盖升级；菜单和桌面图标名称均恰为 `LanSender` / `LanReceiver`，移除中文名称后缀及 GenericName 字段 |
| 构建与目标 | 面向麒麟桌面 V10 SP1 ARM64 / aarch64（飞腾、鲲鹏）；在隔离的 Debian 11 ARM64、glibc 2.31、Rust 1.88.0 容器中完成 release 构建、测试和打包。相关安装验证使用 `--network none`，且未安装 `zenity` 或桌面门户，未下载依赖或在宿主机安装 deb |
| Linux 自动测试 | 105 项库测试与 5 项图像流程集成测试通过，共 110 项；需要 Java 的既有互通测试 1 项为 ignored，本轮未执行 |
| 内置选择器功能与交互 | 文件和目录选择弹窗采用统一深浅主题与按钮样式，增加常用位置导航、当前目录搜索、隐藏文件切换、文件图标、大小和类型信息；支持路径输入、单击选择、双击进入或确认、刷新和取消。窄窗口调整导航排列，底部选择信息与确认按钮固定，仅文件列表虚拟滚动。路径、搜索及 LanSender 补发帧号输入框中的文字和提示文字均垂直居中。选择器定向测试 macOS 8 项、Linux 9 项通过，覆盖选择、确认、取消、中文和空格路径、目录导航、搜索与隐藏文件过滤，以及小视口和深浅主题；新增大窗口缩至小窗口、再缩至 400×360 的首帧与后续帧检查，在文件和目录两种模式、深浅两种主题下均通过。尚未将这些自动交互结果表述为截图视觉验收或物理麒麟 GUI 验收 |
| macOS 回归 | 102 项库测试通过，LanSender / LanReceiver 两个 binary 测试目标成功运行（各 0 项）；macOS 继续使用原生文件对话框。本轮仅生成 Linux 包，现有 macOS 2.0.13 与 Windows 2.0.14 下载包未重新打包 |
| ELF 与包内容 | 两个可执行文件均为 AArch64，最高所需 GLIBC 符号版本为 2.29；最终 deb 内 LanSender / LanReceiver 与更新后的 release 文件逐字节一致，桌面快捷方式 helper 及安装钩子也与源文件一致。菜单名称、命令入口、图标和完整许可证检查通过，无外置 `imgBridge.properties` |
| 断网安装与升级 | 无 `zenity`、`xdg-desktop-portal` 与 `xdg-desktop-portal-gtk` 的 ARM64 容器中，正常执行 `dpkg -i` 将已配置成功的 2.0.15 直接升级为 2.0.16，升级检查通过；另一个容器中首次安装通过。没有使用强制安装选项，包配置与已安装程序的检查通过 |
| 桌面图标真实安装检查 | `verify-desktop-shortcuts.sh` 分别在独立容器内完成 2.0.15 升级与干净安装的全流程。真实非 root 用户的中文 XDG 桌面路径（含空格）、精确应用名、Exec / Icon 字段、0755 权限与所属 UID 检查通过；重复安装、图形安装器普通账号回退、禁用桌面目录、保留用户自定义文件和符号链接、保存用户修改、卸载清理均通过。XDG 配置不执行用户代码；gio trusted 元数据尝试以用户身份进行，后端失败不阻塞安装。最终输入框与动态窗口调整修正未改变这些安装脚本，包内脚本已核验与同一源文件一致，全流程结果沿用，未重复运行 |
| 原生窗口启动 | 最终包覆盖升级后，在 Xvfb + Mesa 软件 OpenGL 环境中重复检查，LanSender 的 1152×800 窗口与 LanReceiver 的 768×800 窗口均映射可见并持续运行 2 秒；该检查仅确认原生窗口启动与短时存活 |
| 实际选择器截图 | 在三个独立的断网 ARM64 容器中移除 zenity 和桌面门户，安装最终 2.0.16 deb 后检查浅色、深色文件选择器及接收端目录选择器。实际查看 PNG，路径、搜索与补发号输入框的提示和内容垂直居中；已打开弹窗真实缩至 640×480 时，标题、关闭与底部确认/取消完整显示。最终截图、原始 PPM、动作记录及进程日志位于 `dist/validation-2.0.16/picker-final-*`；未带 final 的目录保留为修复前对照 |
| 静态与格式检查 | 根项目格式、打包和安装验证 Shell 语法、截图脚本 Python 编译检查通过；macOS 选择器库 Clippy `-D warnings` 通过，本地 rfd 保留 42 项既有上游警告。Linux 构建器未安装 `cargo-clippy`，Linux Clippy 未执行 |
| 验证范围 | 用户已确认前一版 2.0.15 在麒麟机器上可安装使用。本轮 2.0.16 的结果来自自动交互、断网容器安装和 Xvfb 窗口检查，没有物理麒麟桌面 V10 SP1 的本版 GUI、显示驱动或完整屏幕文件传输结果，也未验证传输速率 |

产物为 `dist/linux-arm64/LanSSR_2.0.16_arm64.deb`（8,630,006 字节），SHA256 为 `c2e12ba73f79152f37fe0d6b12184e1fa3744a5421fcf5c893c2c9dfab8f14ff`，并记录于 `dist/SHA256SUMS`。日志与报告位于 `dist/validation-2.0.16/`：`kylin-arm64-build-upgrade.log`、`desktop-shortcuts-upgrade.log`、`clean-install-desktop-gui.log`、`macos-regression.log`、`picker-tests.log`、`picker-clippy.log`、`linux-gui-smoke.log` 和 `package-report.txt`。构建升级、macOS 回归与选择器测试日志分别归档自最终一轮 `/private/tmp/lanssr-2.0.16-final-kylin-build-upgrade.log`、`/private/tmp/lanssr-2.0.16-final-macos-regression.log` 和 `/private/tmp/lan-ssr-picker-resize-tests.log`。

在安装包所在目录执行 `sudo dpkg -i LanSSR_2.0.16_arm64.deb`，即可覆盖升级现有 2.0.15 并生成两个桌面应用图标。文件名前缀更改不改变系统内部包名，卸载仍使用 `sudo apt remove lanbridge`。安装说明位于 `packaging/linux/README.zh-CN.md`，并随 deb 安装至 `/usr/share/doc/lanbridge/README.zh-CN.md`。

2.0.15 麒麟桌面 V10 SP1 ARM64 离线文件选择修复（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 安装问题与修复 | 旧版 2.0.14 在未安装 `zenity` 的机器上无法完成配置。Linux 版改用内置 egui 文件和目录选择器，包内 `Depends` 移除 `zenity`；选择文件或保存目录不再需要外部对话框程序或文件选择门户。桌面门户仅保留为 Wayland 截屏的可选 `Suggests` |
| 构建与目标 | 面向麒麟桌面 V10 SP1 ARM64 / aarch64（飞腾、鲲鹏）；在隔离的 Debian 11 ARM64、glibc 2.31、Rust 1.88.0 容器中完成 release 构建。构建、测试、打包、安装和升级验证使用 `--network none`，未下载依赖或在宿主机安装 deb |
| Linux 自动测试 | 101 项库单元测试与 5 项图像流程集成测试通过，共 106 项；包含 Linux 内置选择器及发送端、接收端按钮流程。需要 Java 的既有互通测试 1 项为 ignored，本轮未执行 |
| 内置选择器交互 | 选择器测试覆盖选择、确认、取消、目录导航、中文和空格路径、Linux 非 UTF-8 文件名、符号链接及错误反馈，确认与取消按钮使用实际 egui 输入验证；400×360 / 640×480 小视口、深浅主题、128 项列表滚动和固定底部按钮通过。发送端取消保留原选择，接收端运行时禁止更改保存目录。目录仅在打开、导航或刷新时读取，列表采用虚拟滚动 |
| macOS 回归 | 98 项库测试通过，LanSender / LanReceiver 两个 binary 测试目标成功运行（各 0 项）；macOS 继续使用原生文件对话框。本轮仅生成 Linux 包，现有 macOS 2.0.13 与 Windows 2.0.14 下载包未重新打包 |
| ELF 与包内容 | 两个可执行文件均为 AArch64，最高所需 GLIBC 符号版本为 2.29；deb 内 LanSender / LanReceiver 与最新 release 文件逐字节一致。两个独立菜单入口、命令入口、图标、许可证及内置默认参数检查通过，无外置 `imgBridge.properties` |
| 断网覆盖升级 | 在一次性 ARM64 容器内移除 `zenity`、`xdg-desktop-portal` 与 `xdg-desktop-portal-gtk`，复现旧包因缺少 `zenity` 配置失败，状态为 `install ok unpacked`；随后正常执行 `dpkg -i` 安装 2.0.15，成功覆盖并配置旧包，未强制安装、未下载依赖 |
| 断网干净安装 | 在另一个同样没有上述组件的断网 ARM64 容器内，2.0.15 首次安装成功。干净安装与覆盖升级均通过 `dpkg --audit`、无缺失库的 `ldd`、已安装命令的 `--help` / `--version` 和从其他工作目录读取内置默认参数的检查 |
| 原生窗口启动 | 在干净安装容器的 Xvfb + Mesa 软件 OpenGL 环境中，LanSender 的 1152×800 窗口与 LanReceiver 的 768×800 窗口均映射可见并持续运行 2 秒；新版移除 Linux rfd 依赖后，本轮窗口烟测成功 |
| 静态与格式检查 | macOS 应用库与两个 binary 的 Clippy `-D warnings`、根项目格式检查及打包/验证 Shell 语法检查通过；本地 rfd 保留 42 项既有上游警告。Linux 构建器未安装 `cargo-clippy`，Linux Clippy 未执行 |
| 验证范围 | 选择器交互使用 egui 输入测试；Xvfb 烟测只确认已安装应用的原生窗口启动、可见与短时存活。本轮没有麒麟桌面 V10 SP1 物理实机的 GUI、显示驱动或完整屏幕文件收发结果，不将容器检查作为实机传输或速率验证 |

产物为 `dist/linux-arm64/lanbridge_2.0.15_arm64.deb`（8,580,170 字节），SHA256 为 `27cc18418e893610be5fb66138f0b44f405cc1878823d4250e4aaeff880b5f58`，并记录于 `dist/SHA256SUMS`。日志与报告位于 `dist/validation-2.0.15/`：`kylin-arm64-build-and-tests.log`、`kylin-arm64-package-upgrade.log`、`kylin-arm64-clean-install-and-gui.log`、`linux-gui-smoke.log`、`macos-regression.log`、`macos-clippy.log` 和 `package-report.txt`。

断网机器在安装包所在目录执行 `sudo dpkg -i lanbridge_2.0.15_arm64.deb`；旧版 2.0.14 因缺少 `zenity` 未完成配置时，可直接覆盖升级，无需先安装 `zenity` 或卸载旧包。安装说明位于 `packaging/linux/README.zh-CN.md`，并随 deb 安装至 `/usr/share/doc/lanbridge/README.zh-CN.md`；其他系统运行库未全部捆绑，精确依赖以包内 `Depends` 为准。

2.0.14 麒麟桌面 V10 SP1 ARM64 安装包（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 构建与目标 | 面向麒麟桌面 V10 SP1 ARM64 / aarch64（飞腾、鲲鹏）；在隔离的 Debian 11 ARM64、glibc 2.31、Rust 1.88.0 容器中完成 release 构建。编译、测试、打包和安装检查均使用 `--network none`，未下载依赖或在宿主机安装软件 |
| 自动测试 | 94 项单元测试和 5 项图像流程集成测试通过，共 99 项；原 Java 互通测试仍为既有可选 ignored 项，本轮未执行 |
| ELF 与包内容 | 两个可执行文件均为 AArch64，最高所需 GLIBC 符号版本为 2.29；从 deb 提取的 LanSender / LanReceiver 与最新 release 文件逐字节一致。项目、rfd 及 egui_glow 许可证与源文件逐字节一致 |
| 双应用与设置 | 保留两个独立应用及菜单入口，desktop-file-validate 通过；中文界面、水印、主题、保存目录、连续接收、历史列表和间隔设置沿用当前版本。无外置 `imgBridge.properties`；包采用兼容较旧 dpkg 的 gzip 压缩 |
| 文件对话框依赖 | 系统桌面门户及其后端为可选 `Suggests`，`zenity` 后备保留在依赖中；离线机器无需为了文件选择另行安装门户。完整系统运行库未捆绑，精确依赖以包内 `Depends` 为准 |
| 离线安装检查 | 在一次性 ARM64 容器内完整执行 `dpkg -i` 并安装成功；`dpkg --audit` 通过，两个程序的 `ldd` 均无缺失链接库，已安装命令的 `--help` / `--version` 通过且版本均为 2.0.14；从其他工作目录启动的内置默认参数检查通过 |
| 实机验证范围 | 本轮未在麒麟桌面 V10 SP1 实机验证 GUI、显示驱动、文件选择对话框或完整屏幕收发。容器安装与 CLI 启动检查不等于目标桌面实机启动、收发或传输速率验证 |

产物为 `dist/linux-arm64/lanbridge_2.0.14_arm64.deb`（9,973,590 字节），SHA256 为 `8834055024bf70298ce55f5471b1257e41b3079f5488dadf9e057ee090b12bc2`。构建日志、包及安装检查日志和汇总报告分别位于 `dist/validation-2.0.14/kylin-arm64-build.log`、`kylin-arm64-package-and-install.log` 和 `kylin-arm64-package-report.txt`。安装说明位于 `packaging/linux/README.zh-CN.md`，并随 deb 安装至 `/usr/share/doc/lanbridge/README.zh-CN.md`；断网机器使用 `sudo dpkg -i lanbridge_2.0.14_arm64.deb`，缺少的依赖请从对应麒麟版本与 ARM64 架构的安装介质或官方软件源带入，避免跨发行版混装。

2.0.14 Windows 7 OpenGL 1.1 兼容渲染（2026-10-12）：

| 项目 | 结果 |
| --- | --- |
| 启动报错原因 | Windows 7 系统 OpenGL 1.1 能建立原生窗口，但原 `egui_glow 0.33.3` 在画家初始化时要求 OpenGL 2.0 以上，触发用户报告的 `egui_glow requires opengl 2.0+` 错误 |
| 自动渲染适配 | 桌面 OpenGL 1.x 自动使用本地固定管线画家，调用系统 OpenGL 1.1 接口、CPU 顶点 / 索引数组及完整原生指针；支持 OpenGL 2.0 以上或 OpenGL ES 时继续使用原着色器路径。发布构建不启用强制兼容的 `legacy-test` 特性 |
| 纹理与水印 | 非 2 的幂纹理填充为 2 的幂内部区域，附加复制边缘的 OpenGL 一像素边框；可用时采用 `CLAMP_TO_EDGE`，纯 1.1 采用 `GL_CLAMP`。查询驱动实际 `GL_TEXTURE_BORDER`，对去掉边框的驱动裁剪局部更新区域，避免负偏移；仅去掉边框且没有边缘钳制能力时，对线性纹理做半像素 UV 钳制以保留白色面板，该特殊分支会微调线性整图的外边缘及触边字形插值。最近邻传输纹理不采用该调整。发送端按系统纹理上限限制格子数量，不重采样水印，仍保留完整的传输格子 |
| 自动测试 | 应用库 94 项测试、无默认特性库 28 项测试及图像流程集成测试 5 项通过；包含纹理上限 1024 的初始画面、容量调整和水印像素保持。兼容画家纹理算法 13 项独立测试通过 |
| 真实软件 OpenGL | Linux ARM64 / Xvfb / Mesa 20.3.5 使用 `GL_VERSION=1.1` 并禁用非 2 的幂纹理扩展，未启用强制兼容特性，实际自动选择固定管线画家。8 项像素检查通过：360×180 普通 / 水印画面逐像素一致且可解码；13×7 局部更新、预乘透明度与剪裁、1024 宽纹理、线性 `WHITE_UV` 面板、左上边缘更新、真实中英文和 1024 宽字体图集通过；GL 错误为 0。容器驱动实际纹理上限为 16384，1024 宽上传不等于实际最大上限为 1024 的驱动测试 |
| 现代渲染回归 | 同一真实驱动在 `GL_VERSION=2.1` 时自动选择原着色器路径，上述 8 项检查全部通过；中文 / 拉丁字体渲染得到 2020 个有效文字像素，实色面板保持准确 |
| 无边缘钳制能力的诊断分支 | 额外禁用 Mesa 的 `GL_EXT_texture_edge_clamp` / `GL_SGIS_texture_edge_clamp`，驱动仍将纹理边框去掉；半像素线性 UV 兼容处理后，8 项检查全部通过，普通 / 水印传输画面仍逐像素一致。字体得到 2352 个文字像素，触边字形插值有微调，不宣称与正常边缘钳制路径字体像素一致。此模拟条件不等同于真实保留纹理边框的 Microsoft GDI |
| 字体视觉检查 | 已保存 `dist/validation-2.0.14/font-panel-1.1.png`、`font-panel-no-edge-clamp.png` 和 `font-panel-2.1.png`；正常 1.1 与现代 2.1 字体面板逐像素一致。实际查看无边缘钳制分支及现代 2.1 的 PNG，中英文均清晰可读、字形完整、背景颜色无异常，保留上述字体像素数量差异说明 |
| 静态与格式检查 | 应用库及 LanSender / LanReceiver 两个 binary 的 Clippy `-D warnings`、根项目及本地 egui_glow 的格式检查、Windows 打包 Shell 语法和校验脚本 Python 编译通过；本地 rfd 保留 42 项既有上游警告 |
| Windows 分发 | 两个独立 exe 继续使用 `x86_64-win7-windows-msvc`、静态 CRT、PE OS / GUI 子系统 6.1；Windows 7 基线的普通 / delay 导入检查通过。六文件白名单、ZIP CRC、release / 包内二进制一致性和许可证一致性通过，没有新增外部图形 DLL |
| 上游许可证 | 包内增加本地 egui_glow 的完整 MIT 许可证；来源、校验值及补丁范围记录在 `vendor/egui_glow/PATCHES.md` |
| 完整 Linux GUI 尝试 | 完整 GUI 烟测在当前 Linux 容器中受 XCB / XInitThreads 启动断言阻塞，LanSender 在映射窗口前退出，未完成两应用的完整 GUI 流程；测试用线程初始化 preload 也未解决。此结果不作为 Windows 窗口启动或完整 GUI 已验证的证据 |

Windows 通用包为 `dist/LanBridge-2.0.14-windows-x64.zip`（5,422,810 字节），SHA256 为 `1720883f95b65d2ce004bc71ffff949ac989f694038346ae4d812784b0fa2b27`，并保存于 `dist/SHA256SUMS`。报告与日志位于 `dist/validation-2.0.14/`，像素验证的复现方法见 `scripts/validation-legacy-glow/README.md`。macOS 现有发布包仍为 `dist/LanBridge-2.0.13-macos-arm64.zip`，本轮未重新打包。Linux 软件 OpenGL 的实际像素测试验证兼容画家的渲染行为，并不等于 Windows 7 实机启动、文件对话框或完整屏幕传输验证；本轮没有 Windows 7 / 10 / 11 实机结果，也不作实机传输速率保证。

2.0.13 Windows x64 通用包（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| 单包分发 | 同一个 `windows-x64` ZIP、同一套 LanSender.exe / LanReceiver.exe 覆盖 Windows 7 SP1 / Windows 10 / Windows 11；本版本生成包含两个独立 exe 的通用包 |
| 默认构建 | `scripts/package-windows.sh` 默认使用 `x86_64-win7-windows-msvc`、匹配的 Rust 1.98.1 标准库源码、静态 CRT 和 GUI 6.01。旧 `--win7` 参数验证为同一构建的别名；默认及别名都生成相同包路径 |
| 系统适配 | 只读审查确认 DPI 等现代接口在运行时检测；Win7 使用兼容方式，Win10/11 保留可用的新接口。捕获路径使用 GDI，文件对话框和桌面路径使用 ole32 的 CoTaskMemFree |
| 自动审计 | 打包完成后自动执行 Windows 7 基线的严格 PE、普通 / delay 导入和 ZIP 审计。两端 PE OS / 子系统为 6.1，无已知较新必需接口、动态 CRT 或 WebView2 依赖；旧 2.0.12 普通 Windows 包因新系统必需导入被正确拒绝 |
| 定向验证 | Windows capture 7 项、窗口布局 2 项测试通过；缺失 DPI 接口的降级、线程上下文恢复、物理坐标和捕获载荷覆盖通过。Shell 语法、Python 编译与 Rust 格式检查通过 |
| 包与资源 | release、应用目录与 ZIP 内 exe 的图标七尺寸、载荷和 2.0.13.0 版本核验通过；五文件白名单、CRC、许可证、release 一致性和 SHA256 通过 |

通用包为 `dist/LanBridge-2.0.13-windows-x64.zip`。报告、默认构建及别名命令日志、定向测试记录位于 `dist/validation-2.0.13/`，校验值位于 `dist/SHA256SUMS`。本轮未在 Windows 7 / 10 / 11 实机运行；Win7 仍需支持程序所需现代 OpenGL 的显卡厂商驱动，系统默认 OpenGL 1.1 不够。macOS 双应用包已恢复为 2.0.13 并完成 ARM64、签名和 ZIP 内容核验；麒麟 deb 沿用双应用清单。

2.0.12 简约应用图标（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| 设计 | 按参考图采用浅米白圆角底、鼠尾草绿前框、深灰绿细线和两个叠放方框。发送端箭头朝右上，接收端箭头朝左下。实际查看了浅深背景下 16～256 像素预览 |
| 资源 | 256 / 1024 像素透明 PNG、7 个尺寸 ICO 和含 Retina 的 16～1024 像素 ICNS 均可解码；生成脚本重跑结果逐字节一致。Linux SVG 与源文件一致 |
| Windows | 普通版、Win7 版的 release、应用目录及 ZIP 内 exe 均核验通过：RT_GROUP_ICON / RT_ICON 载荷与对应新版 ICO 一致，两端图标不同，版本为 2.0.12.0；五文件白名单、CRC、许可证和适用平台静态导入审计通过 |
| macOS | CFBundleIconFile 指向对应新版 ICNS，原包及解压后的图标资源与源文件一致；整体本地签名、ARM64 架构、macOS 11.0 最低系统标记和 CLI 启动检查通过 |

三个新版 ZIP 位于 `dist/LanBridge-2.0.12-windows-x64.zip`、`dist/LanBridge-2.0.12-windows7-x64.zip` 和 `dist/LanBridge-2.0.12-macos-arm64.zip`。报告、日志与图标对比图位于 `dist/validation-2.0.12/`，校验值位于 `dist/SHA256SUMS`。Windows 实机 GUI 未验证；macOS 使用本地临时签名，尚未公证。本轮未重复完整屏幕收发测试，也未重打旧版麒麟 deb。

2.0.11 应用图标（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| 图标设计 | 两端使用统一的屏幕、文件和棋盘格元素；LanSender 为蓝色向上箭头，LanReceiver 为青绿色向下箭头。已实际查看浅深背景下 16～256 像素预览 |
| 原生窗口 | 两端在启动时加载各自内嵌的 256 像素 PNG；图标不在截图、编码或传输循环中处理 |
| Windows | 普通版和 Win7 版均按 binary 分别嵌入 7 个尺寸的 ICO；RT_GROUP_ICON、RT_ICON 载荷逐字节与对应源 ICO 一致，两端图标不同，VERSIONINFO 为 2.0.11.0。构建不新增依赖或资源编译器 |
| macOS | 两个应用分别包含 ICNS 和 CFBundleIconFile，覆盖 16～1024 像素及 Retina 尺寸；sips/Pillow 可解析。原包和解压后资源与源文件一致，整体本地签名验证通过 |
| Linux | 两个桌面入口的 SVG 已同步为新版；旧 2.0.1 deb 未重新打包 |
| 自动检查 | 格式与全目标 Clippy 检查通过；本地 rfd 保留 42 项既有警告。本轮仅改图标与打包资源，未重复协议测试和完整屏幕收发测试 |

Windows 10 / 11 x64、Windows 7 SP1 x64 和 macOS ARM64 三个 2.0.11 ZIP 已重新构建，内容白名单、CRC、许可证、release 一致性和 SHA256 核验通过；Windows 适用平台静态导入审计、macOS 解压后签名与 CLI 启动检查通过。报告和日志位于 `dist/validation-2.0.11/`，校验值位于 `dist/SHA256SUMS`。Windows 实机 GUI 未验证；macOS 最低 11.0、本地签名且未公证。

2.0.10 提示、文件名与速度左右对齐（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| LanSender | 等待提示及发送状态中的文件名固定在状态行左侧，当前速度固定在同一行右侧；长状态截断显示，传输画面位置保持稳定 |
| LanReceiver | 等待接收提示及文件名固定在传输信息行左侧，当前速度固定在右侧；长文件名可悬停查看，保留加粗进度条与历史列表 |
| 布局原因与修正 | 固定大小标签容器默认居中；改为明确的左右两列，分别使用左向和右向布局，避免仅设文字对齐仍居中的问题 |
| 验证 | Sender 21 项、Receiver UI 10 项定向测试通过，增强既有多分辨率、高 DPI、深浅主题和长文本布局断言；全目标 Clippy 与格式检查通过。本轮仅改对齐，未重复协议与图像流程测试 |

版本为 2.0.10，日志与分发报告位于 `dist/validation-2.0.10/`。Windows 10 / 11 x64、Windows 7 SP1 x64 与 macOS ARM64 三个 ZIP 已构建，CRC、内容白名单、许可证、release 一致性及 SHA256 核验通过；Windows 适用平台静态导入审计和 macOS 解压后签名、CLI 启动通过。使用说明已加入左右对齐说明。macOS 最低 11.0、本地签名且未公证；Windows 实机 GUI 未验证。窗口比例、速度统计与传输时序沿用 2.0.9。

2.0.9 当前速度与接收进度条（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| 当前速度 | 两端显示 B/s、KB/s、MB/s 或 GB/s，按最近约 1 秒实际载荷统计；发送端记录完成展示的数据帧，补发计入，校准和信息帧不计入；接收端基于唯一有效字节计数，重复帧不计入 |
| 性能与时间 | 固定 64 条样本、无动态样本队列，接收事件携带原始工作线程时间戳，界面批量处理不会虚增速度。速度统计不增加载荷复制、文件读取、截图、帧等待或重绘定时器；发送端速度文字最多每 250ms 更新，接收端沿用原有 100ms 刷新 |
| 状态处理 | 等待、停止、完成或重新校准后速度为零；持续无新数据时近期样本到期，速度自然归零；发送暂停不继续统计旧帧 |
| 进度条 | LanReceiver 进度条由 12 加粗到 20 逻辑像素，等待、寻找发送端和停止时隐藏蓝色填充，收到文件信息后显示进度；保留完成进度与本次接收历史 |
| 布局验证 | 两端深浅主题、不同 DPI 和工作区比例测试通过；接收端速度与文件名同一行，长速度文字截断并可悬停查看；缺帧和错误信息、滚动历史保持可见。视觉形状测试确认等待时没有蓝点，传输中出现蓝色进度 |
| 自动检查 | 最新源码通过 89 个单元测试和 5 个图像流程集成测试；全目标 Clippy 和格式检查通过。新增验证覆盖精准末帧载荷、补发、去重、工作线程时间戳、暂停和停滞。Java 可选互通测试本轮未执行；本地 rfd 保留 42 项上游警告 |
| Windows 分发 | 普通 Windows 10 / 11 x64 与 Windows 7 SP1 x64 包均完成 release 构建，ZIP 内容白名单、CRC、许可证、二进制一致性和 SHA256 通过；普通包通过适用平台静态审计，Win7 包通过已知不兼容系统导入检查 |
| macOS 分发 | Apple Silicon ARM64、最低 macOS 11.0；ZIP 内容、CRC、中文说明、许可证、本地签名及解压后签名、CLI 启动和 release 代码/数据一致性通过；未进行 Apple 公证 |

源码版本为 2.0.9，日志与分发报告位于 `dist/validation-2.0.9/`。产物为 `dist/LanBridge-2.0.9-windows-x64.zip`、`dist/LanBridge-2.0.9-windows7-x64.zip` 与 `dist/LanBridge-2.0.9-macos-arm64.zip`，校验值位于 `dist/SHA256SUMS`。本轮未进行 Windows 实机 GUI 或屏幕传输速率实测，以上验证不构成实机吞吐量保证。

2.0.8 间隔设置、窗口比例与动态容量（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| 发送间隔 | 设置页提供 30 / 50 / 100 毫秒，默认 50 毫秒，并注明速度、稳定性与丢帧时的选择建议；正常数据帧使用所选间隔，校准与信息帧至少 1 秒、补发至少 200 毫秒；发送或准备文件期间禁止修改 |
| 窗口布局 | Sender 靠左，占工作区约 60% 宽、80% 高；Receiver 靠右，占约 40% 宽、80% 高。尺寸包括原生窗框，并按任务栏、macOS 菜单栏/Dock、DPI 和多显示器工作区调整；控件必要时缩小，历史列表独立滚动 |
| 传输容量 | 传输画面采用 2 物理像素格子，动态行列数均为 4 的倍数，最低 160×80 格，长宽比 1.2～3.2。640×336 格测试画面每帧载荷 107514 字节，约为原基础画面的 3 倍；不作为实机吞吐量保证 |
| 重新校准 | 移动窗口或改变传输画面尺寸后，发送端重新分片并从头发送；接收端在校准期间确认完整画面边界，边界变更后重建接收容量，无法确认时停止解析旧区域，避免混用分片 |
| 自动检查 | 最新源码通过 78 个单元测试和 5 个图像流程集成测试；包含间隔切换、高 DPI 布局、动态定位、大容量逐字节还原与重新定位。格式检查和全目标 Clippy 通过；Java 可选互通测试本轮未执行。macOS 本地 rfd 有 42 项上游警告，项目自身 Clippy 无错误 |
| Windows 10 / 11 x64 分发 | 两个 exe 为 PE32+ AMD64 / GUI 子系统、静态 CRT、无需 WebView2；ZIP 白名单、CRC、许可证、release 二进制一致性及 SHA256 检查通过 |
| Windows 7 SP1 x64 分发 | 独立兼容目标构建，两个 exe 的 OS / 子系统版本 6.1；ZIP、CRC、release 一致性、许可证和已知 Win7 不兼容导入审计通过。普通 Windows 包的现代标准库接口不适用于 Win7，需使用专用包 |

产物为 `dist/LanBridge-2.0.8-windows-x64.zip`、`dist/LanBridge-2.0.8-windows7-x64.zip` 和 `dist/LanBridge-2.0.8-macos-arm64.zip`。日志与审计报告位于 `dist/validation-2.0.8/`，校验值位于 `dist/SHA256SUMS`。发送间隔说明同时写入 Windows 包说明与两个 macOS 应用的 `Contents/Resources/使用说明.txt`。

请同时更新 LanSender 与 LanReceiver，并保持两端水印开关一致；2.0.7 及更早接收端不支持新版动态容量画面的自动定位。本轮尚未进行 Windows 实机 GUI 与完整屏幕传输验证，也未在 Windows 7 实机验证显卡初始化和文件对话框；静态导入审计仅排除已知装载问题。

2.0.7 macOS ARM64 构建（2026-10-10）：

| 项目 | 结果 |
| --- | --- |
| 构建与平台 | 使用已安装的 Command Line Tools 和当前 2.0.7 源码完成 Release 构建；两个应用均为 Apple Silicon ARM64，Mach-O 与应用最低系统标记均为 macOS 11.0；未在 macOS 11 实机验证 |
| 功能与参数 | 无新的行为改动；保留 1440×800 基础画面、360×200 数据格子、自适应整数格子缩放与大画面至少 100ms 的数据帧停留，两端水印默认开启 |
| 自动测试 | 沿用 2.0.7 源码已通过的 65 个单元测试与 3 个图像流程集成测试，共 68 个；本次只打包，未重复运行测试 |
| CLI 启动 | 两个应用及 ZIP 解压后的 `--help` / `--version` 启动检查通过 |
| 签名与权限 | 两个完整应用及 ZIP 解压后的本地签名、可执行权限与 Info.plist 检查通过；动态依赖仅为 macOS 系统库；未进行 Apple 公证 |
| ZIP 与许可证 | 最终 ZIP 的文件白名单、CRC、原应用与解压后文件一致性及 SHA256 检查通过；两个应用均随附本项目许可证与本地 rfd 的 MIT 许可证；无外置配置文件 |
| GUI 与收发 | 本次未重新进行 GUI 点击与真实屏幕完整收发验证，不将 CLI 启动检查作为 GUI 实测 |

产物为 `dist/LanBridge-2.0.7-macos-arm64.zip`，最终构建日志与验证报告位于 `dist/validation-2.0.7/macos-build.log` 和 `dist/validation-2.0.7/macos-build-report.txt`，报告另保存于 `dist/macos-build-report.txt`，校验值位于 `dist/SHA256SUMS`。请同时更新 LanSender 与 LanReceiver，并保持两端水印模式一致；旧版接收端不能识别自适应缩小后的 2 像素水印格子。LanReceiver 接收前仍需屏幕录制授权。

2.0.7 Windows 7 SP1 x64 兼容版（2026-10-09）：

| 项目 | 结果 |
| --- | --- |
| 原版启动失败原因 | 对 2.0.6 两个 exe 的真实导入表检查发现 `bcryptprimitives.dll!ProcessPrng`、`WaitOnAddress` / `WakeByAddress*` 和 `combase.dll!CoTaskMemFree`；接收端另静态导入 `SetThreadDpiAwarenessContext`。这些 Windows 7 缺少的依赖可在进入应用代码前阻止装载 |
| 标准库与随机数 | 使用 Rust 1.98.1 的 `x86_64-win7-windows-msvc` 目标，从对应版本源码构建标准库，选择 Windows 7 兼容的随机数和同步接口；标准库源码下载 SHA256 为 `5c846ebcebcc7e2e0777a4cdaa12051691593f16a7e94edbae5e6241cc62d98c` |
| DPI 兼容 | 接收线程动态检测现代线程 DPI 接口，缺失时继承进程策略；GUI 使用 winit 的旧系统降级，CLI 启用 `SetProcessDPIAware`。缺接口、作用域恢复和设置失败路径测试通过 |
| COM 兼容 | 文件选择对话框与默认桌面路径处理直接导入 `ole32.dll!CoTaskMemFree`；保留本地 rfd 原 MIT 许可证。最终 PE 未导入 `combase.dll` |
| 功能保留 | 窗口按工作区与 DPI 自适应，接收端持续接收、区域 GDI 捕获与缓冲复用、大画面 100ms 数据帧停留和中文界面保留 |
| 自动检查 | 65 个单元测试与 3 个图像流程集成测试通过；macOS 全目标及 Win7 目标两个程序的 Clippy 通过，格式检查通过。原 Java 互通测试本轮未重跑；本地 rfd 在 macOS 上有上游弃用/unused_unsafe 警告，不影响构建 |
| 分发检查 | 两个 exe 均为 PE32+ / AMD64、GUI 子系统，OS / 子系统版本 6.1，静态 CRT；完整常规和 delay 导入表、已知较新 DLL/接口、五文件 ZIP 白名单、CRC、许可证、release 文件一致性与 SHA256 检查通过。验证脚本能拒绝原 2.0.6 包的四类已知阻断 |

产物为 `dist/LanBridge-2.0.7-windows7-x64.zip`，日志与静态审计报告位于 `dist/validation-2.0.7/`，校验值位于 `dist/SHA256SUMS`。请完整解压并同时替换两个 exe。无需 WebView2、Java 或另装 Visual C++ 运行库；需 Windows 7 SP1 64 位和支持程序所需现代 OpenGL 的显卡厂商驱动，系统自带 OpenGL 1.1 不足。

尚未进行 Windows 7 实机启动、显卡初始化、文件对话框与完整收发验证。静态检查排除了已知装载阻断，并非完整 Windows 7 API 白名单或实机兼容保证；旧普通 Windows 包和 macOS 包未重打。

2.0.6 Windows 高分辨率接收优化（2026-10-09）：

| 项目 | 结果 |
| --- | --- |
| 性能问题 | 旧接收循环在定位后仍逐帧截取整个桌面，4K 每帧 BGRA 数据约 33MB，随后全屏转换和区域复制；解析结束后又额外等待轮询间隔 |
| Windows 区域捕获 | 初次定位与失效重定位使用全屏截图；定位后只截取棋盘格，复用 DC、top-down DIB 和 RGB 缓冲；1440×800 区域位图约 4.6MB，避免每帧重新创建全屏位图 |
| 捕获正确性 | 物理坐标和负坐标副屏边界、越界/溢出拒绝、BGRA 通道与行方向、复用像素缓冲、2/4 像素灰度/水印帧逐字节还原通过；Windows GUI/CLI 的定位与区域截屏均使用物理 DPI 作用域 |
| 捕获失效 | 桌面客户端区域捕获错误时重新定位，不提交旧缓冲；手动区域错误仍明确报错。Windows 资源释放与编译检查通过，未在目标系统运行 |
| 发送时序 | 实际画面大于 720×400 时数据帧至少展示 100ms，小画面仍使用内置 30ms；补发至少 200ms，界面显示当前有效间隔；容量和解码平均采样规则不变 |
| 轮询时序 | GUI/CLI 等待扣除本轮截图与解析耗时，处理超过周期时不额外等待；计时回归通过 |
| 采样时序模型 | 模拟 51～80ms 不等的采样周期与多个起始相位；原 30ms 停留漏帧，100ms 停留下 3MiB / 16MiB 文件均完整保存且逐字节一致。此模型不代表 Windows 实机截图时钟 |
| 自动检查 | 62 个单元测试与 3 个图像流程集成测试通过；Clippy 和格式检查通过；原 Java 互通测试未重跑 |
| Windows x64 包 | PE32+ / AMD64、GUI 子系统、静态 CRT、系统 DLL 依赖、release 文件一致性、ZIP 白名单/CRC 与 SHA256 检查通过 |

图像处理基准使用本机 macOS ARM64 Release，64 次采样，不含操作系统截图、桌面刷新或文件保存时间。水印模式、相同的 1440×800 画面下，中位耗时如下；区域输入路径用于隔离图像处理成本，不是 Windows GDI 的实测性能：

| 桌面分辨率 | 旧全屏转换＋裁剪＋解码 | 模拟区域输入转换＋解码 |
| --- | --- | --- |
| 1920×1080 | 3.433ms | 2.654ms |
| 3840×2160 | 4.157ms | 2.734ms |

完整 8 个定位/解析场景及分位数据见 `dist/validation-2.0.6/receive-benchmark.tsv`，可用 `cargo run --release --locked --no-default-features --example receive_benchmark -- 64` 重跑。产物为 `dist/LanBridge-2.0.6-windows-x64.zip`，报告和日志位于 `dist/validation-2.0.6/`。请同步更新两个 exe；Windows 实机 4K 连续收发尚未验证，macOS 本轮未重新打包。

2.0.5 Windows 窗口自适应修复（2026-10-09）：

| 项目 | 结果 |
| --- | --- |
| 工作区适配 | 按 Windows 当前窗口所在屏幕的工作区、原生边框、任务栏与 DPI 调整内窗尺寸和控件缩放，保留多屏工作区原点 |
| 布局回归 | 模拟原生窗口与延迟 UI 缩放，1920×1080 的 100% / 125% / 150% / 200% / 300% 缩放、1366×768、竖屏、顶部/左侧任务栏、负坐标副屏以及移到大屏通过；完整控件和棋盘格均位于可用区内，稳定后不反复调整窗口 |
| 显示与数据 | 水印格子从 4 像素缩到 2 像素时，实际画面为 720×400，360×200 格和每帧 35994 字节载荷不变；最近邻缩放后的定位、信息帧、多帧文件与尾帧逐字节还原通过 |
| 移动与补发 | 活跃传输的位置或格子大小改变时重新校准并从第 1 帧完整发送；停止后移动画面再补发也完整重发；整数物理坐标避免浮点误差造成重复重启 |
| 空间不足 | 视口不完整、画面移出屏幕工作区或最小化时暂停且不绘制局部棋盘格；极小工作区仍需调整窗口或显示缩放 |
| 自动检查 | 55 个单元测试、3 个图像流程集成测试通过；Clippy 和格式检查通过；原 Java 互通测试未重跑 |
| Windows x64 分发 | 两个 exe 的 PE32+ / AMD64、GUI 子系统、系统 DLL 依赖、release 文件一致性通过；ZIP 白名单、CRC 和 SHA256 通过 |

产物为 `dist/LanBridge-2.0.5-windows-x64.zip`，日志与报告位于 `dist/validation-2.0.5/`。请同步更新两个 exe，旧接收端不支持 2 像素水印格子。Windows 实机运行尚未验证；macOS 本轮未重新打包，现有包仍为 2.0.4。

验证环境：macOS Apple Silicon，Rust 1.98.1，Java/Javac 17。构建使用已安装的 Command Line Tools，未改变系统 Xcode 设置。

2.0.4 macOS ARM64 验证（2026-10-09）：

| 验证 | 结果 |
| --- | --- |
| 内置参数 | 两端同步恢复为 1440×800、格子大小 4、帧间隔 30 毫秒，水印默认开启；版本为 2.0.4 |
| LanReceiver 启动位置 | 根据主屏幕边界和系统缩放计算，窗口默认靠屏幕右侧打开；macOS/Linux 使用 xcap 返回的逻辑坐标，Windows 使用物理坐标换算 |
| LanReceiver 水印开关 | 主界面顺序为“开始接收 → 水印 → 停止接收”；接收或停止处理中禁用，完全停止后可切换；设置页已移除水印卡片 |
| 自动测试 | 52 个单元测试与 3 个图像流程集成测试通过；Clippy 和格式检查通过 |
| macOS ARM64 分发 | 两个 `.app` 为 ARM64，最低系统 11.0；本地签名、解压后签名、权限、`--help` 启动和系统动态库检查通过；ZIP CRC、内容范围、应用文件一致性和 SHA256 校验通过；未进行 Apple 公证 |

报告和日志位于 `dist/validation-2.0.4/`；本版本 macOS 与 Windows 客户端默认尺寸同步为 1440×800，不能与旧版 720×400 客户端混用。

2.0.4 Windows x64 验证（2026-10-09）：

| 验证 | 结果 |
| --- | --- |
| 内置参数 | 两端同步为 1440×800、格子大小 4、帧间隔 30 毫秒，水印默认开启 |
| LanReceiver 界面 | 启动位置按主屏幕右侧计算；水印滑动开关位于“开始接收”和“停止接收”之间，设置页不再显示水印卡片 |
| Windows x64 分发 | 两个 exe 为 PE32+ / AMD64、GUI 子系统，静态 MSVC CRT；仅依赖系统 DLL，无 WebView2 或 Visual C++ 运行库依赖 |
| ZIP 验证 | 白名单、CRC、打包文件与 release 二进制一致性和 SHA256 校验通过 |

报告和日志位于 `dist/validation-2.0.4/`；Windows 实机尚未验证。

| 验证 | 结果 |
| --- | --- |
| `cargo build --locked --release` | 通过，生成两个原生可执行程序 |
| `cargo test --locked --all-targets` | 52 个单元测试、3 个完整图像流程集成测试通过 |
| Java 可选互通测试 | 通过：原 Java 与 Rust 双向文件/文本、灰度/水印，PNG 逐像素一致 |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all -- --check` | 通过 |
| macOS 发送端界面 | 已验证 2.0.1 独立设置页、浅色与深色主题切换、水印开启/关闭、返回主界面、中文控件和完整传输画面 |
| 接收客户端 | 固定 860×730 点窗口；深浅主题、1×/2× DPI、长文本下历史位置不变与独立滚动通过；设置页导航、主题切换、文件名点击定位路径通过交互测试 |
| 连续接收 | 重复同名文件、空文件、保持末帧不重复保存、校准后重启、部分文件补发与保存失败重试通过状态机测试 |
| 原生连续接收 | 新客户端尚未完成跨窗口的真实屏幕连续收发验证；Windows 未实机验证 |
| 发送端布局 | 深浅主题、1×/2× DPI、640/720 点宽度、长文件名与状态通过；空间不足时暂停且不显示部分格子，鼠标悬停不会产生覆盖格子的提示 |
| 真实屏幕自动定位 | 已定位 1440×800 和 640×320 画面，均为 4 像素格子 |
| 真实屏幕帧解析 | 已解析信息帧和数据帧，检测并报告缺失帧 |
| 完整原生桌面文件恢复 | 用户提供的真实接收日志显示 2/2 帧完成；已在本机确认收到的 39231 字节文件与桌面原文件逐字节一致 |
| Windows / Linux 原生运行 | 未实机验证 |

2.0.3 Windows 与 macOS 包验证（2026-10-09）：

| 验证 | 结果 |
| --- | --- |
| 发送端布局 | 待发送文件区域保留整行，发送按钮保持左侧、停止和重置保持右侧，水印滑动开关放在发送按钮行中间；设置页只保留主题设置 |
| 水印默认值 | 发送端和接收端均默认开启；发送期间主界面水印开关禁用；切换会重建网格、清除旧传输并保留所选文件 |
| 画面尺寸 | 两端保持 720×400 物理像素、4 像素格子、30 毫秒帧间隔 |
| 自动测试 | 52 个单元测试与 3 个图像流程集成测试通过；浅色/深色、1×/2× DPI、640/720 点宽度、长文件名和状态下的布局与完整传输画面通过；Clippy 和格式检查通过 |
| macOS 界面 | 已实际查看恢复后的整行文件选择布局、居中的水印开关及 720×400 传输画面，并验证开关和设置页导航 |
| Windows x64 分发 | 两个 exe 的 PE32+ / AMD64、GUI 子系统、系统 DLL 依赖检查通过；ZIP 白名单、CRC、打包文件与 release 二进制一致性及 SHA256 校验通过 |
| macOS ARM64 分发 | 两个应用均为 2.0.3 / ARM64，最低系统为 11.0，动态依赖仅为 macOS 系统库；完整应用的本地签名、解压后的签名、可执行权限和 `--help` 启动通过；ZIP 内容范围、CRC、应用文件一致性和 SHA256 校验通过，未进行 Apple 公证 |

报告和日志位于 `dist/validation-2.0.3/`；Windows 实机尚未验证。请使用本包配套的两个客户端，并保持两端水印开关一致。

2.0.2 Windows 包验证（2026-10-09）：

| 验证 | 结果 |
| --- | --- |
| 内置画面尺寸 | 发送端与接收端默认同步为 720×400；格子大小 4 像素、帧间隔 30 毫秒、水印默认关闭；每帧载荷 8994 字节 |
| 自动测试 | 52 个单元测试与 3 个图像流程集成测试通过；新默认尺寸下 1× DPI 窄窗口暂停、2× DPI 完整显示通过；Clippy 与格式检查通过 |
| Windows x64 分发 | 两个 exe 的 PE32+ / AMD64、GUI 子系统、系统 DLL 依赖检查通过；ZIP 白名单、CRC、打包文件一致性与 SHA256 校验通过 |

报告和日志位于 `dist/validation-2.0.2/`；Windows 实机尚未验证。旧 2.0.1 接收端的 1440×800 自动定位参数与新发送画面不同，使用新版发送端时需同步接收端。

2.0.1 验证（2026-10-09）：

| 验证 | 结果 |
| --- | --- |
| 内置参数 | 固定 1440×800、cellSize=4、interval=30，waterMask 默认 false；Rust 启动不读取配置文件，两端 `--config` 参数已删除 |
| 水印切换 | 设置页点击测试通过；发送端重建编码网格、清空旧传输与补发状态并保留所选文件；接收端更新定位及手动区域，接收期间禁止更改 |
| macOS / ARM64 Linux 自动测试 | 均通过 52 个单元测试和 3 个图像流程集成测试；Clippy 与格式检查通过 |
| 新版 Windows x64 ZIP | PE32+ / AMD64、GUI 子系统、系统 DLL 依赖检查通过；ZIP 白名单、CRC 和 SHA256 通过，仅包含两个 exe、LICENSE、使用说明 |
| 新版 ARM64 deb | AArch64 ELF、最高 GLIBC 符号版本 2.29；无配置文件或配置链接；容器安装、不同启动目录下的内置参数启动和 GUI 窗口启动通过 |
| macOS 应用 | 两个应用重打包为 2.0.1 / ARM64，最低系统标记为 11.0，应用内无外置配置文件；两个应用完整本地签名验证通过，ZIP 解压后的权限、签名和 `--help` 启动通过；LanSender 设置页已在本机实际点击验证 |

新版构建及测试日志在 `dist/validation-2.0.1/` 和 `dist/linux-arm64/validation-2.0.1/`。麒麟 deb 仍声明系统运行依赖；离线机器必须已有这些依赖，或准备与目标麒麟版本匹配的离线依赖包。

2.0.0 分发包验证（2026-10-09，外置配置移除前）：

| 产物 | 验证结果 |
| --- | --- |
| Windows x64 便携 ZIP | 两个 exe 均为 AMD64 PE32+、GUI 子系统；MSVC CRT 静态链接，仅导入 Windows 系统 DLL，不需要 WebView2；ZIP 文件白名单、CRC 与 SHA256 校验通过 |
| 麒麟 V10 ARM64 deb | 使用 Rust 1.88.0、Debian bullseye ARM64 / glibc 2.31 构建；两个程序均为 AArch64 ELF，实际最高 GLIBC 符号需求为 2.29；采用 gzip 包格式 |
| ARM64 自动测试 | 49 个单元测试、3 个图像流程集成测试通过；Java 互通测试在该容器中未执行 |
| deb 安装检查 | 隔离 ARM64 容器内安装成功；两个 PATH 命令、`--help`、应用菜单入口、配置链接和从其他目录启动时的默认配置查找通过 |
| ARM64 图形启动 | 安装后的两个客户端在 Xvfb / 软件 OpenGL 中生成可见窗口并持续运行；发送端 960×640、接收端 860×730；未在该环境进行真实屏幕收发 |

Windows 和麒麟目标实机尚未验证。麒麟具体 SP 版本、桌面门户与硬件图形驱动仍需在目标机器验证；建议使用 X11 桌面。Windows 包验证报告位于 `dist/windows-build-report.txt`，ARM64 构建、测试、安装及图形启动日志位于 `dist/linux-arm64/`。分发包 SHA256 位于 `dist/SHA256SUMS`。

图像流程集成测试覆盖：定位截图中的校准画面、裁剪数据画面、两种编码模式、整数格子缩放、中文文件名、末帧不满、乱序、重复、丢帧后补发，以及保存后逐字节对比原文件。文本测试验证 UTF-8、换行和中文恢复；自动测试不会改写用户系统剪贴板。

自动桌面测试使用临时目录内生成的测试文件。另根据用户完成的收发操作，核对了接收文件与源文件的一致性。正式使用时应让整个发送区域持续可见，并按接收端缺失帧报告补发。
