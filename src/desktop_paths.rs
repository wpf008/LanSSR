//! Desktop destinations and native file manager actions shared by the clients.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Format paths for users while retaining the original path for filesystem operations.
pub fn display_path(path: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        windows_path_for_display(&path.to_string_lossy()).into_owned()
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.display().to_string()
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_path_for_display(path: &str) -> std::borrow::Cow<'_, str> {
    // Canonical UNC paths need their leading pair of backslashes restored.
    if path
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        return std::borrow::Cow::Owned(format!(r"\\{}", &path[8..]));
    }
    if let Some(dos) = path.strip_prefix(r"\\?\") {
        let bytes = dos.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\'
        {
            return std::borrow::Cow::Borrowed(dos);
        }
    }
    // Other verbatim namespaces, such as volume GUID paths, require their prefix.
    std::borrow::Cow::Borrowed(path)
}

/// Resolve the user's desktop, including a redirected Windows desktop.
pub fn default_desktop_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(desktop) = windows_desktop_dir() {
        return desktop;
    }

    let home = user_home_dir();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let config_dir = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        if let Ok(config) = std::fs::read_to_string(config_dir.join("user-dirs.dirs"))
            && let Some(desktop) = desktop_from_xdg_config(&config, &home)
        {
            return desktop;
        }
    }
    home.join("Desktop")
}

fn user_home_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    let candidates = ["USERPROFILE", "HOME"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["HOME", "USERPROFILE"];

    candidates
        .into_iter()
        .find_map(|name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Open a directory in Finder, Explorer, or the system file manager.
pub fn open_folder(path: &Path) -> Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("无法打开目录：{}", display_path(path)))?;
    if !path.is_dir() {
        bail!("不是目录：{}", display_path(&path));
    }
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    command.arg(file_manager_path(&path));
    launch_file_manager(command)
}

/// Reveal a received file; Linux opens its containing directory.
pub fn reveal_file(path: &Path) -> Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("找不到文件：{}", display_path(path)))?;
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg("-R").arg(path);
        launch_file_manager(command)
    }
    #[cfg(target_os = "windows")]
    {
        let mut selection = std::ffi::OsString::from("/select,");
        selection.push(file_manager_path(&path));
        let mut command = Command::new("explorer.exe");
        command.arg(selection);
        launch_file_manager(command)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        open_folder(path.parent().unwrap_or(&path))
    }
}

fn launch_file_manager(mut command: Command) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动系统文件管理器")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// Windows canonicalization produces verbatim paths; Explorer expects DOS/UNC paths.
#[cfg(target_os = "windows")]
fn file_manager_path(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<_> = path.as_os_str().encode_wide().collect();
    let unc_prefix: Vec<_> = r"\\?\UNC\".encode_utf16().collect();
    let dos_prefix: Vec<_> = r"\\?\".encode_utf16().collect();
    if let Some(unc) = wide.strip_prefix(unc_prefix.as_slice()) {
        let mut result = vec![b'\\' as u16, b'\\' as u16];
        result.extend_from_slice(unc);
        PathBuf::from(OsString::from_wide(&result))
    } else if let Some(dos) = wide.strip_prefix(dos_prefix.as_slice()) {
        PathBuf::from(OsString::from_wide(dos))
    } else {
        path.to_path_buf()
    }
}

#[cfg(not(target_os = "windows"))]
fn file_manager_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(target_os = "windows")]
fn windows_desktop_dir() -> Option<PathBuf> {
    use std::ffi::{OsString, c_void};
    use std::os::windows::ffi::OsStringExt;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            folder: *const Guid,
            flags: u32,
            token: *mut c_void,
            path: *mut *mut u16,
        ) -> i32;
    }
    // Other bindings declare this symbol in combase.dll. Specify the ole32
    // export directly so linking selects the DLL available on Windows 7.
    #[link(name = "ole32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: *mut c_void);
    }

    const DESKTOP: Guid = Guid {
        data1: 0xb4bfcc3a,
        data2: 0xdb2c,
        data3: 0x424c,
        data4: [0xb0, 0x29, 0x7f, 0xe9, 0x9a, 0x87, 0xc6, 0x41],
    };
    let mut raw = std::ptr::null_mut();
    // The API allocates a null-terminated UTF-16 path using the COM allocator.
    let result = unsafe { SHGetKnownFolderPath(&DESKTOP, 0, std::ptr::null_mut(), &mut raw) };
    if result < 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return None;
    }
    let mut length = 0;
    unsafe {
        while *raw.add(length) != 0 {
            length += 1;
        }
        let path = OsString::from_wide(std::slice::from_raw_parts(raw, length));
        CoTaskMemFree(raw.cast());
        Some(PathBuf::from(path))
    }
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
fn desktop_from_xdg_config(config: &str, home: &Path) -> Option<PathBuf> {
    config.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        if key.trim() != "XDG_DESKTOP_DIR" {
            return None;
        }
        let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
        let path = if value == "$HOME" || value == "${HOME}" {
            home.to_path_buf()
        } else if let Some(relative) = value
            .strip_prefix("$HOME/")
            .or_else(|| value.strip_prefix("${HOME}/"))
        {
            home.join(relative)
        } else if !value.contains(['$', '`', '\\']) {
            PathBuf::from(value)
        } else {
            return None;
        };
        path.is_absolute().then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_display_paths_preserve_drive_unc_and_special_destinations() {
        for (source, expected) in [
            (r"\\?\C:\Users\wpf\Desktop", r"C:\Users\wpf\Desktop"),
            (r"\\?\D:\文件 空格\桌面", r"D:\文件 空格\桌面"),
            (r"\\?\UNC\server\share\桌面", r"\\server\share\桌面"),
            (r"\\?\unc\server\share\file.txt", r"\\server\share\file.txt"),
            (r"C:\Users\wpf\Desktop", r"C:\Users\wpf\Desktop"),
            (r"\\server\share\桌面", r"\\server\share\桌面"),
            (r"\\?\Volume{example}\桌面", r"\\?\Volume{example}\桌面"),
            (r"\\.\Device", r"\\.\Device"),
            (r"\\?\C:relative", r"\\?\C:relative"),
            ("/home/用户/桌面", "/home/用户/桌面"),
        ] {
            assert_eq!(windows_path_for_display(source), expected);
        }
        let tail = format!("C:\\{}\\接收文件.txt", "long-folder\\".repeat(30));
        let original = format!(r"\\?\{tail}");
        assert!(original.len() > 260);
        assert_eq!(windows_path_for_display(&original), tail);
        assert!(original.starts_with(r"\\?\"));
    }

    #[test]
    fn xdg_desktop_supports_localized_and_redirected_paths() {
        let home = if cfg!(target_os = "windows") {
            Path::new("C:/Users/用户")
        } else {
            Path::new("/home/用户")
        };
        assert_eq!(
            desktop_from_xdg_config("XDG_DESKTOP_DIR=\"$HOME/桌面\"", home),
            Some(home.join("桌面"))
        );
        assert_eq!(
            desktop_from_xdg_config("XDG_DESKTOP_DIR=\"${HOME}/Desktop\"", home),
            Some(home.join("Desktop"))
        );
        let redirected = if cfg!(target_os = "windows") {
            "D:/work/Desktop"
        } else {
            "/media/work/Desktop"
        };
        assert_eq!(
            desktop_from_xdg_config(&format!("XDG_DESKTOP_DIR=\"{redirected}\""), home),
            Some(PathBuf::from(redirected))
        );
    }

    #[test]
    fn xdg_desktop_ignores_comments_other_keys_and_shell_commands() {
        let home = if cfg!(target_os = "windows") {
            Path::new("C:/Users/user")
        } else {
            Path::new("/home/user")
        };
        let config = "# XDG_DESKTOP_DIR=\"/wrong\"\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n  XDG_DESKTOP_DIR = \"$HOME/Desktop\"\n";
        assert_eq!(
            desktop_from_xdg_config(config, home),
            Some(home.join("Desktop"))
        );
        for invalid in [
            "relative/Desktop",
            "$(touch /tmp/file)",
            "`pwd`",
            "$OTHER/Desktop",
        ] {
            assert_eq!(
                desktop_from_xdg_config(&format!("XDG_DESKTOP_DIR=\"{invalid}\""), home),
                None
            );
        }
    }

    #[test]
    fn file_actions_reject_missing_paths_before_launching() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.txt");
        assert!(reveal_file(&missing).is_err());
        assert!(open_folder(&missing).is_err());
        let file = directory.path().join("test.txt");
        std::fs::write(&file, b"test").unwrap();
        assert!(open_folder(&file).is_err());
    }
}
