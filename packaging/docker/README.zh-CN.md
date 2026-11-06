# 麒麟 ARM64 构建环境

在项目根目录执行：

```sh
sh scripts/build-kylin-arm64.sh
```

需要运行中的 Docker Desktop / Docker，支持 Linux ARM64 容器；x86_64 主机还需要 Docker 的 ARM64 模拟支持。脚本将构建原生 Rust 客户端、执行全部 Rust 测试、生成 `dist/linux-arm64/LanSSR_2.0.16_arm64.deb`，再检查 Debian 包结构、应用入口及最高 GLIBC 符号要求。它不会在宿主机上安装 deb。下载文件名前缀为 LanSSR，系统内部包名和安装路径仍为 lanbridge，确保已有 2.0.15 可直接升级。

Linux 版内置文件和目录选择器，不依赖 `zenity` 或文件选择门户。桌面门户及其后端仅列为可选 `Suggests`，供 Wayland 截屏环境使用；离线安装无需为了选择文件另装这些组件。

## 缓存与离线构建

脚本把宿主 Cargo 的 crate 缓存与索引合并到 `target/linux-cargo/registry`，Linux 编译输出位于 `target/linux-arm64`。宿主缓存默认为 `CARGO_HOME` 或 `~/.cargo`；可以通过 `LANBRIDGE_HOST_CARGO_HOME` 指定其他完整 Cargo 缓存目录。

当项目所有依赖已经缓存时，可以禁止 Cargo 下载：

```sh
sh scripts/build-kylin-arm64.sh --offline
```

`--offline` 仅约束 Cargo；首次构建容器镜像仍需下载 Debian 依赖。已经存在本地构建镜像时，可同时跳过镜像构建，实现完整的本地重建：

```sh
sh scripts/build-kylin-arm64.sh --offline --reuse-builder
```

默认构建镜像名为 `lanbridge-kylin-arm64-builder`，可用 `LANBRIDGE_BUILDER_IMAGE` 替换。`LANBRIDGE_BUILD_JOBS` 设置并行编译任务数，默认为 4；`LANBRIDGE_VERSION` 改变 deb 的版本字段和文件名。离线模式缺少某个 crate 时，Cargo 会明确报出缺失依赖；准备完整缓存或去掉 `--offline` 后重试即可。

## 官方基础镜像下载备用方案

基础镜像默认使用 Docker 官方 Rust 镜像的 Amazon ECR 公共镜像：`public.ecr.aws/docker/library/rust:1.88.0-bullseye`。如果 Docker daemon 无法访问注册表，但宿主的 HTTPS 下载可用，执行：

```sh
python3 scripts/fetch-rust-arm64-image.py
docker load --input target/build-tools/rust-arm64-image.tar
sh scripts/build-kylin-arm64.sh --offline
```

下载器固定 Linux ARM64 的官方 manifest，并检查配置、每个压缩层和展开后的 SHA256。构建脚本在默认基础镜像尚未导入且本地 tar 已存在时，也会自动导入该 tar。下载过程不修改 Docker 的全局镜像源或 TLS 配置；`LANBRIDGE_BASE_IMAGE` 可指定另一个经验证的 ARM64 基础镜像。

## 兼容基础与构建依赖

容器采用 Debian bullseye、GLIBC 2.31 和 Rust 1.88.0。Debian bullseye-security 的包已经迁移到 `archive.debian.org/debian-security`，Dockerfile 使用官方归档与系统 Debian keyring，避免旧 security pool 的 404。

当前 `xcap` / `libspa` 需要比 bullseye 自带 PipeWire 0.3.19 更新的 SPA 常量。构建器仅安装 PipeWire 0.3.65 的编译头文件，源包 SHA256 由 `install-pipewire-headers.py` 验证；链接仍使用 bullseye 的系统运行库。这样不会因更新整个 Linux 基础环境而提高 GLIBC 要求。最终运行库依赖由 `dpkg-shlibdeps` 根据真实 ELF 生成。

Dockerfile 包含 XCB、EGL、GBM、OpenGL、xkbcommon 等开发依赖。GBM 是 `libwayshot` 的链接依赖，即使应用在 X11 模式下截屏也需要在构建环境中提供。

`verify-deb.sh` 的 `--install-smoke` 选项可在单独的隔离构建容器内安装并检查程序命令入口、`--help` 启动及内置默认参数，无需外置配置文件。自动构建默认仅检查包结构；目标麒麟桌面的启动、显示驱动、文件对话框和连续屏幕接收仍需实机验证。

`verify-desktop-shortcuts.sh NEW.deb [OLD.deb]` 仅在可丢弃的容器内测试安装后的桌面图标：包含非 root 账号、中文 XDG 桌面路径、文件权限和所属用户、直接升级、重复安装、图形安装器场景，以及保留用户修改过的快捷方式。`verify-kylin-offline.sh NEW.deb [OLD.deb]` 会在无 zenity、无桌面门户的容器中验证离线安装及旧包升级。
