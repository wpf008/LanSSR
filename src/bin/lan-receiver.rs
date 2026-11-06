#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use imgbridge::{
    config::Config,
    desktop_paths,
    receiver::{self, ReceiverOptions},
    receiver_ui,
};

#[derive(Parser)]
#[command(
    name = "LanReceiver",
    version,
    about = "持续接收屏幕格子传输的文件，默认打开桌面客户端"
)]
struct Args {
    /// 文件保存目录；默认使用系统桌面目录。
    #[arg(long)]
    output_dir: Option<PathBuf>,
    /// 使用单次接收的终端模式。
    #[arg(long)]
    cli: bool,
    /// 终端模式覆盖已有的同名文件；客户端自动编号另存。
    #[arg(long)]
    overwrite: bool,
    /// 显示器序号，可用 --list-monitors 查看。
    #[arg(long)]
    monitor: Option<usize>,
    /// 手动指定截图像素区域：x,y,width,height,cell_size；需同时指定 --monitor。
    #[arg(long, requires = "monitor")]
    region: Option<String>,
    /// 列出可用显示器。
    #[arg(long)]
    list_monitors: bool,
    /// 截屏轮询间隔，单位为毫秒。
    #[arg(long, default_value_t = 5)]
    poll_ms: u64,
    /// 接收总超时，单位为秒；0 表示持续等待。
    #[arg(long, default_value_t = 0)]
    timeout: u64,
    /// 接收大小上限，单位为字节；默认 512 MiB。
    #[arg(long, default_value_t = 536_870_912)]
    max_bytes: usize,
}

fn main() -> ExitCode {
    let terminal_mode = std::env::args_os().skip(1).any(|arg| {
        matches!(
            arg.to_str(),
            Some("--cli" | "--list-monitors" | "--help" | "-h" | "--version" | "-V")
        )
    });
    imgbridge::platform_startup::run("LanReceiver", terminal_mode, start)
}

fn start() -> Result<()> {
    let args = Args::parse();
    if args.list_monitors {
        return receiver::list_monitors();
    }
    let config = Config::default();
    let region = args
        .region
        .as_deref()
        .map(|value| receiver::parse_region(value, config.water_mask))
        .transpose()?;
    let options = ReceiverOptions {
        output_dir: args
            .output_dir
            .unwrap_or_else(desktop_paths::default_desktop_dir),
        overwrite: args.overwrite,
        monitor: args.monitor,
        region,
        poll_ms: args.poll_ms,
        timeout_seconds: args.timeout,
        max_bytes: args.max_bytes,
    };
    if args.cli {
        receiver::run(config, options)
    } else {
        receiver_ui::run(config, options)
    }
}
