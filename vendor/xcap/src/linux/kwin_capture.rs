//! The compositor's native, noninteractive legacy screenshot interface.
//! UKUI V10 SP1 uses its own service/interface names, distinct from KDE's.
use std::{fs, path::Path};

use image::{ImageReader, RgbaImage};
use scopeguard::defer;
use zbus::blocking::{Connection, Proxy};

use crate::{XCapError, XCapResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Endpoint {
    pub service: &'static str,
    pub interface: &'static str,
}

pub(super) const ENDPOINTS: [Endpoint; 3] = [
    Endpoint {
        service: "org.ukui.KWin",
        interface: "org.ukui.kwin.Screenshot",
    },
    Endpoint {
        service: "org.ukui.kwin",
        interface: "org.ukui.kwin.Screenshot",
    },
    Endpoint {
        service: "org.kde.KWin",
        interface: "org.kde.kwin.Screenshot",
    },
];

pub(super) fn available(conn: &Connection, endpoint: Endpoint) -> XCapResult<bool> {
    let bus = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )?;
    Ok(bus.call("NameHasOwner", &(endpoint.service,))?)
}

pub(super) fn screenshot(
    conn: &Connection,
    endpoint: Endpoint,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> XCapResult<RgbaImage> {
    if width <= 0 || height <= 0 {
        return Err(XCapError::new("截图区域大小必须大于零"));
    }
    let proxy = Proxy::new(conn, endpoint.service, "/Screenshot", endpoint.interface)?;
    // The string-returning overload captures directly. Older FD overloads
    // start interactive selection and cannot be used for a stream of frames.
    let filename: String = proxy.call("screenshotArea", &(x, y, width, height, false))?;
    if filename.is_empty() {
        return Err(XCapError::new(
            "UKUI/KWin 返回了空的截图路径，请确认系统截图特效已启用",
        ));
    }
    let path = Path::new(&filename);
    if !path.is_absolute() {
        return Err(XCapError::new("UKUI/KWin 返回的截图路径不是绝对路径"));
    }
    // Clean up compositor-generated temporary screenshots, including failed
    // decodes. Never remove arbitrary paths outside the temporary directory.
    let temporary = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(std::env::temp_dir().canonicalize().ok())
        .is_some_and(|(parent, directory)| parent == directory)
        && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file());
    defer! { if temporary { let _ = fs::remove_file(path); } }
    let image = ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?
        .to_rgba8();
    if image.dimensions() != (width as u32, height as u32) {
        return Err(XCapError::new(format!(
            "UKUI/KWin 截图尺寸与请求不一致：收到 {}×{}，请求 {width}×{height}",
            image.width(),
            image.height()
        )));
    }
    Ok(image)
}
