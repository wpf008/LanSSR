use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/lansender.ico");
    println!("cargo:rerun-if-changed=assets/icons/lanreceiver.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var_os("CARGO_FEATURE_DESKTOP").is_none()
    {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must provide the package version");
    for (binary, icon) in [("LanSender", "lansender"), ("LanReceiver", "lanreceiver")] {
        let icon_path = format!("assets/icons/{icon}.ico");
        let ico =
            fs::read(&icon_path).unwrap_or_else(|err| panic!("Cannot read {icon_path}: {err}"));
        let resources = icon_resources(&ico, &icon_path);
        let resource_file = out_dir.join(format!("{icon}.res"));
        let mut res = Vec::new();
        // A .res starts with a null resource header. MSVC and lld-link both
        // accept this format directly, including when cross-building on macOS.
        append_resource(&mut res, 0, 0, 0, &[]);
        for (id, data) in resources.images {
            append_resource(&mut res, 3, id, 0x1030, &data);
        }
        append_resource(&mut res, 14, 1, 0x1030, &resources.group);
        append_resource(&mut res, 16, 1, 0x0030, &version_resource(binary, &version));
        fs::write(&resource_file, res).expect("Cannot write Windows application resources");
        let link_file = linkable_resource(&resource_file);
        // Select one binary explicitly so each executable receives its own
        // primary icon, rather than both executables sharing the first icon.
        println!("cargo:rustc-link-arg-bin={binary}={}", link_file.display());
    }
}

struct IconResources {
    images: Vec<(u16, Vec<u8>)>,
    group: Vec<u8>,
}

fn icon_resources(ico: &[u8], path: &str) -> IconResources {
    assert!(ico.len() >= 6, "Invalid ICO header in {path}");
    assert_eq!(read_u16(ico, 0), 0, "Invalid ICO reserved field in {path}");
    assert_eq!(read_u16(ico, 2), 1, "Expected an ICO image in {path}");
    let count = read_u16(ico, 4);
    assert!(count > 0, "No image sizes in {path}");
    assert!(
        ico.len() >= 6 + usize::from(count) * 16,
        "Truncated ICO directory in {path}"
    );
    let mut group = ico[..6].to_vec();
    let mut images = Vec::with_capacity(usize::from(count));
    for index in 0..count {
        let entry = 6 + usize::from(index) * 16;
        let length = read_u32(ico, entry + 8) as usize;
        let offset = read_u32(ico, entry + 12) as usize;
        let end = offset
            .checked_add(length)
            .expect("ICO image offset overflow");
        assert!(
            length > 0 && offset >= 6 + usize::from(count) * 16 && end <= ico.len(),
            "Truncated ICO image in {path}"
        );
        let id = index + 1;
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
        images.push((id, ico[offset..end].to_vec()));
    }
    IconResources { images, group }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn align_dword(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}

fn append_resource(res: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    align_dword(res);
    res.extend_from_slice(&(u32::try_from(data.len()).expect("Resource too large")).to_le_bytes());
    res.extend_from_slice(&32_u32.to_le_bytes());
    // Ordinal resource type and name: 0xffff followed by the integer ID.
    for value in [0xffff_u16, kind, 0xffff, id] {
        res.extend_from_slice(&value.to_le_bytes());
    }
    res.extend_from_slice(&0_u32.to_le_bytes()); // DataVersion
    res.extend_from_slice(&flags.to_le_bytes());
    res.extend_from_slice(&(if kind == 0 { 0_u16 } else { 0x0409 }).to_le_bytes());
    res.extend_from_slice(&0_u32.to_le_bytes()); // Version
    res.extend_from_slice(&0_u32.to_le_bytes()); // Characteristics
    res.extend_from_slice(data);
    align_dword(res);
}

fn version_resource(binary: &str, version: &str) -> Vec<u8> {
    let parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| {
            part.parse()
                .expect("Package version must start with major.minor.patch")
        })
        .collect();
    assert_eq!(parts.len(), 3, "Package version must have three components");
    let version_ms = u32::from(parts[0]) << 16 | u32::from(parts[1]);
    let version_ls = u32::from(parts[2]) << 16;
    let fixed_info: Vec<u8> = [
        0xfeef04bd_u32,
        0x00010000,
        version_ms,
        version_ls,
        version_ms,
        version_ls,
        0x3f,
        0,
        0x00040004, // VOS_NT_WINDOWS32
        1,          // VFT_APP
        0,
        0,
        0,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    let description = if binary == "LanSender" {
        "LanSender - screen file sender"
    } else {
        "LanReceiver - screen file receiver"
    };
    let mut strings = Vec::new();
    for (key, value) in [
        ("FileDescription", description),
        ("FileVersion", version),
        ("InternalName", binary),
        ("OriginalFilename", &format!("{binary}.exe")),
        ("ProductName", "LanBridge"),
        ("ProductVersion", version),
    ] {
        let text = utf16(value);
        strings.push(version_block(key, 1, (text.len() / 2) as u16, &text, &[]));
    }
    let table = version_block("040904b0", 1, 0, &[], &strings);
    let string_info = version_block("StringFileInfo", 1, 0, &[], &[table]);
    let translation = version_block("Translation", 0, 4, &[0x09, 0x04, 0xb0, 0x04], &[]);
    let variable_info = version_block("VarFileInfo", 1, 0, &[], &[translation]);
    version_block(
        "VS_VERSION_INFO",
        0,
        fixed_info.len() as u16,
        &fixed_info,
        &[string_info, variable_info],
    )
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn version_block(
    key: &str,
    kind: u16,
    value_length: u16,
    value: &[u8],
    children: &[Vec<u8>],
) -> Vec<u8> {
    let mut block = vec![0, 0];
    block.extend_from_slice(&value_length.to_le_bytes());
    block.extend_from_slice(&kind.to_le_bytes());
    block.extend_from_slice(&utf16(key));
    align_dword(&mut block);
    block.extend_from_slice(value);
    for child in children {
        align_dword(&mut block);
        block.extend_from_slice(child);
    }
    let length = u16::try_from(block.len()).expect("Windows version block too large");
    block[..2].copy_from_slice(&length.to_le_bytes());
    block
}

fn linkable_resource(resource: &Path) -> PathBuf {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        return resource.to_owned();
    }
    // GNU linkers need a COFF object. The normal MinGW toolchain includes
    // windres; MSVC builds never need an external resource compiler.
    let target = env::var("TARGET").expect("Cargo must provide TARGET");
    let compiler = env::var_os(format!("RC_{target}"))
        .or_else(|| env::var_os(format!("RC_{}", target.replace('-', "_"))))
        .or_else(|| env::var_os("RC"))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "windres".into()
            } else {
                let arch = target.split('-').next().unwrap();
                format!("{arch}-w64-mingw32-windres").into()
            }
        });
    let object = resource.with_extension("o");
    let status = Command::new(compiler)
        .args(["--input-format=res", "--output-format=coff", "--input"])
        .arg(resource)
        .arg("--output")
        .arg(&object)
        .status()
        .expect("Cannot run MinGW windres to link the Windows application icon");
    assert!(
        status.success(),
        "Cannot convert Windows resources to a COFF object"
    );
    object
}
