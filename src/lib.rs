//! imgBridge's wire protocol and screen codec, independent of a desktop session.
pub mod config;
pub mod grid;
pub mod locator;
pub mod protocol;
pub mod receive;

#[cfg(feature = "desktop")]
pub mod receiver;
#[cfg(feature = "desktop")]
pub mod sender;

#[cfg(feature = "desktop")]
pub mod desktop_paths;
#[cfg(feature = "desktop")]
pub mod platform_startup;
#[cfg(feature = "desktop")]
pub mod receiver_ui;

#[cfg(feature = "desktop")]
mod ui_style;

#[cfg(feature = "desktop")]
mod transfer_texture;

#[cfg(feature = "desktop")]
pub mod transfer_speed;

#[cfg(feature = "desktop")]
mod window_layout;

#[cfg(feature = "desktop")]
mod file_picker;

#[cfg(all(feature = "desktop", any(target_os = "windows", test)))]
mod windows_capture;
