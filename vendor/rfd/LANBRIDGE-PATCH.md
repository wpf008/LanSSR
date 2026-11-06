# Local compatibility patch

This is rfd 0.16.0, distributed under its original MIT LICENSE.

In `src/backend/win_cid/file_dialog/com.rs`, the COM file-name allocation is
freed using `CoTaskMemFree` imported directly from `ole32.dll` with Rust's
`raw-dylib` linkage, explicitly selecting ole32 over bindings to the newer DLL.
Windows 7 exports this function from ole32, while the current windows-sys binding imports it from
`combase.dll`, a Windows 8 library. No dialog behavior or allocator changes.
