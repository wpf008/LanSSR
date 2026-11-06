use libwayshot_xcap::{Error, WayshotConnection};

fn verify_app_startup_monitor_path(scale: f32) {
    // Same calls as src/window_layout.rs::initial_screen_area. Xvfb supplies
    // real XCB/RandR data while the legacy Wayland fixture supplies scaling.
    let monitors = xcap::Monitor::all().expect("startup must enumerate XCB monitors");
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .expect("Xvfb must expose a monitor");
    assert_eq!(monitor.scale_factor().unwrap(), scale);
    assert_eq!((monitor.x().unwrap(), monitor.y().unwrap()), (0, 0));
    assert_eq!(
        (monitor.width().unwrap(), monitor.height().unwrap()),
        ((1280.0 / scale) as u32, (720.0 / scale) as u32)
    );
    println!("PASS actual xcap startup scale/geometry path, scale={scale}");
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    assert_eq!(arguments.len(), 3);
    let expectation = &arguments[0];
    let output_version: u32 = arguments[1].parse().unwrap();
    let xdg_version: u32 = arguments[2].parse().unwrap();
    let scale = if expectation == "missing-xdg" || expectation == "no-outputs" {
        1.0
    } else {
        2.0
    };
    verify_app_startup_monitor_path(scale);
    let connection = WayshotConnection::new();
    if expectation == "missing-xdg" {
        assert!(
            matches!(
                connection,
                Err(Error::Bind(_)) | Err(Error::ProtocolNotFound(_))
            ),
            "{connection:?}"
        );
        println!("PASS missing xdg-output reports a Result error");
        return;
    }
    if expectation == "no-outputs" {
        assert!(
            matches!(connection, Err(Error::NoOutputs)),
            "{connection:?}"
        );
        println!("PASS absent outputs report NoOutputs");
        return;
    }
    let connection = connection.expect("output enumeration must succeed without panic");
    let outputs = connection.get_all_outputs();
    assert_eq!(outputs.len(), 1);
    let output = &outputs[0];
    assert_eq!(
        (output.physical_size.width, output.physical_size.height),
        (32, 16)
    );
    let region = output.logical_region.inner;
    assert_eq!((region.position.x, region.position.y), (7, 11));
    assert_eq!((region.size.width, region.size.height), (16, 8));
    if output_version >= 4 || xdg_version >= 2 {
        assert_eq!(output.name, "LEGACY-1");
        assert_eq!(output.description, "Legacy Wayland test output");
    } else {
        assert!(
            !output.name.is_empty(),
            "v1 output still needs a usable fallback name"
        );
    }
    let image = connection.screenshot_single_output(output, false);
    if expectation == "capture-fails" {
        assert!(matches!(image, Err(Error::FramecopyFailed)), "{image:?}");
        println!("PASS failed screencopy returns without hanging");
        return;
    }
    if expectation == "missing-copy" {
        assert!(
            matches!(image, Err(Error::ProtocolNotFound(_))),
            "{image:?}"
        );
        println!("PASS missing screencopy reports a Result error");
        return;
    }
    if expectation == "unknown-format" {
        assert!(
            matches!(image, Err(Error::NoSupportedBufferFormat)),
            "{image:?}"
        );
        println!("PASS unknown SHM format returns an error without hanging");
        return;
    }
    let image = image.expect("SHM screen capture must succeed").to_rgba8();
    assert_eq!(image.dimensions(), (32, 16));
    for y in 0..16 {
        for x in 0..32 {
            assert_eq!(
                image.get_pixel(x, y).0,
                [19 + x as u8, 41 + y as u8, 73, 255]
            );
        }
    }
    // Repeated captures and output refresh check object lifetime and queue reuse.
    for _ in 0..3 {
        let next = connection
            .screenshot_single_output(output, false)
            .unwrap()
            .to_rgba8();
        assert_eq!(next, image);
    }
    let mut connection = connection;
    connection.refresh_outputs().unwrap();
    assert_eq!(connection.get_all_outputs().len(), 1);
    println!(
        "PASS dimensions, logical position, name, all 512 SHM pixels, repeated captures and refresh"
    );
}
