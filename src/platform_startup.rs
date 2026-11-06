//! Keep Windows desktop launches free of a console while preserving explicit CLI use.

use std::process::ExitCode;

/// Set the context limit before app creation (eframe supplies it in the first
/// event-loop input later). The initial sender image must obey it too.
pub(crate) fn initialize_graphics_limits(cc: &eframe::CreationContext<'_>) {
    use eframe::glow::HasContext as _;
    if let Some(gl) = &cc.gl {
        let maximum = unsafe { gl.get_parameter_i32(eframe::glow::MAX_TEXTURE_SIZE) };
        if maximum > 0 {
            cc.egui_ctx.input_mut(|input| {
                input.max_texture_side = maximum as usize;
                input.raw.max_texture_side = Some(maximum as usize);
            });
        }
    }
}

pub fn run(
    title: &str,
    terminal_mode: bool,
    start: impl FnOnce() -> anyhow::Result<()>,
) -> ExitCode {
    initialize_cli_dpi(terminal_mode);
    attach_parent_console();
    match start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let detail = format!("{error:#}");
            let message = format!("{title} 启动失败：\n{}", startup_error_detail(&detail));
            eprintln!("{message}");
            if !terminal_mode {
                show_startup_error(title, &message);
            }
            ExitCode::FAILURE
        }
    }
}

/// Add an actionable hint for the old renderer error if it is ever surfaced.
/// The bundled painter supports OpenGL 1.1; stale executables or an unusable
/// graphics environment should not be diagnosed as an unsupported GL version.
fn startup_error_detail(detail: &str) -> String {
    let normalized = detail.to_ascii_lowercase().replace([' ', ':'], "");
    // Different eframe/glutin versions use either “require” or “requires”
    // and may include a colon between the renderer and the OpenGL message.
    // Match the stable parts so both desktop binaries show the same useful
    // diagnosis instead of exposing the raw backend wording.
    if normalized.contains("egui_glow") && normalized.contains("opengl2.0+") {
        format!(
            "{detail}\n\n当前图形环境无法初始化界面渲染。本版已包含 OpenGL 1.1 兼容支持，请确认已完整解压最新版本并同时替换 LanSender.exe 和 LanReceiver.exe。如果仍无法启动，请安装对应的 Windows 7 显卡驱动并重启；在虚拟机或远程桌面中，可尝试启用 3D 加速或改用本机会话。"
        )
    } else {
        detail.to_owned()
    }
}

#[cfg(target_os = "windows")]
fn initialize_cli_dpi(terminal_mode: bool) {
    if terminal_mode {
        // CLI capture has no winit event loop to establish the process policy.
        // This Vista/Win7 API prevents virtualized screenshot coordinates.
        // GUI launches leave the policy to winit's per-monitor-aware fallback.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn initialize_cli_dpi(_terminal_mode: bool) {}

#[cfg(target_os = "windows")]
fn attach_parent_console() {
    // Explorer launches have no parent console. Command-line launches with
    // arguments can attach to their terminal for --help, --version and --cli.
    if std::env::args_os().nth(1).is_some() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        unsafe {
            let _ = AttachConsole(u32::MAX);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn attach_parent_console() {}

#[cfg(target_os = "windows")]
fn show_startup_error(title: &str, message: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            owner: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            flags: u32,
        ) -> i32;
    }
    let wide = |value: &str| {
        value
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let title = wide(title);
    let message = wide(message);
    unsafe {
        let _ = MessageBoxW(std::ptr::null_mut(), message.as_ptr(), title.as_ptr(), 0x10);
    }
}

#[cfg(not(target_os = "windows"))]
fn show_startup_error(_title: &str, _message: &str) {}

#[cfg(test)]
mod tests {
    use super::startup_error_detail;

    #[test]
    fn explains_legacy_windows_opengl_context() {
        let detail =
            startup_error_detail("无法打开窗口：egui_glow:OpenGL:egui_glow require opengl 2.0+ ");
        assert!(detail.contains("本版已包含 OpenGL 1.1 兼容支持"));
        assert!(detail.contains("Windows 7 显卡驱动"));
    }

    #[test]
    fn accepts_eframe_requires_spelling() {
        let detail = startup_error_detail("egui_glow requires opengl 2.0+");
        assert!(detail.contains("同时替换 LanSender.exe 和 LanReceiver.exe"));
    }

    #[test]
    fn leaves_unrelated_startup_errors_unchanged() {
        assert_eq!(startup_error_detail("窗口创建失败"), "窗口创建失败");
    }
}
