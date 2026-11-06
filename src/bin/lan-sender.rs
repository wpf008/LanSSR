#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use imgbridge::config::Config;

/// 通过屏幕格子发送文件。
#[derive(Parser)]
#[command(name = "LanSender", version, about = "通过屏幕格子发送文件")]
struct Args {
    /// 预选文件；点击“发送文件”后才开始传输。
    #[arg(short, long)]
    file: Option<PathBuf>,
}

fn main() -> ExitCode {
    imgbridge::platform_startup::run("LanSender", false, start)
}

fn start() -> Result<()> {
    let args = Args::parse();
    imgbridge::sender::run(Config::default(), args.file)
}
