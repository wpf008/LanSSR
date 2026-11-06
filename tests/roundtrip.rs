//! Exercise the boundary between screen location, pixels, wire frames, and
//! ordered reconstruction without depending on a desktop capture service.

use image::{Rgb, RgbImage, imageops::FilterType};
use imgbridge::{
    config::Config,
    grid::Grid,
    locator::{LocatedGrid, find_grid},
    protocol::{Frame, Transfer, TransferKind, parse_resend},
    receive::{ReceiveEvent, ReceiveSession},
};

struct ScreenTransfer {
    sender_grid: Grid,
    location: LocatedGrid,
    screen: RgbImage,
}

impl ScreenTransfer {
    fn new(water_mask: bool, observed_cell: u32) -> Self {
        let config = Config {
            width: 80,
            height: 48,
            cell_size: 4,
            water_mask,
            ..Config::default()
        };
        Self::with_config(config, observed_cell)
    }

    fn with_config(config: Config, observed_cell: u32) -> Self {
        Self::with_receiver_config_at(config.clone(), &config, observed_cell, 41, 35)
    }

    fn with_receiver_config_at(
        config: Config,
        receiver: &Config,
        observed_cell: u32,
        x: u32,
        y: u32,
    ) -> Self {
        let sender_grid = Grid::new(
            config.width,
            config.height,
            config.cell_size,
            config.water_mask,
        )
        .unwrap();
        let panel_width = config.width / config.cell_size * observed_cell;
        let panel_height = config.height / config.cell_size * observed_cell;
        let calibration = image::imageops::resize(
            &sender_grid.checkerboard(),
            panel_width,
            panel_height,
            FilterType::Nearest,
        );
        let mut screen = RgbImage::from_pixel(
            panel_width + x + 76,
            panel_height + y + 58,
            Rgb([96, 113, 137]),
        );
        place(&mut screen, &calibration, x, y);
        let location = find_grid(&screen, receiver).expect("locate calibration screen");
        assert_eq!((location.x, location.y), (x, y));
        assert_eq!(location.grid.cell_size, observed_cell);
        assert_eq!(
            (location.grid.width, location.grid.height),
            (panel_width, panel_height)
        );
        assert_eq!(
            location.grid.payload_capacity(),
            sender_grid.payload_capacity()
        );
        Self {
            sender_grid,
            location,
            screen,
        }
    }

    fn capture_frame(&mut self, frame: Frame) -> Frame {
        let source = self.sender_grid.encode(&frame.encode().unwrap()).unwrap();
        // This is the same scaling used by a remote display for calibration
        // and payload, rather than independently generating scaled frames.
        let displayed = image::imageops::resize(
            &source,
            self.location.grid.width,
            self.location.grid.height,
            FilterType::Nearest,
        );
        place(
            &mut self.screen,
            &displayed,
            self.location.x,
            self.location.y,
        );
        let captured = image::imageops::crop_imm(
            &self.screen,
            self.location.x,
            self.location.y,
            self.location.grid.width,
            self.location.grid.height,
        )
        .to_image();
        let bytes = self.location.grid.decode(&captured).unwrap();
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(decoded, frame);
        decoded
    }

    fn session(&self, max_bytes: usize) -> ReceiveSession {
        ReceiveSession::new(self.location.grid.payload_capacity(), max_bytes).unwrap()
    }
}

#[test]
fn dynamic_screen_geometry_increases_payload_and_reconstructs_files_without_manual_configuration() {
    for water_mask in [false, true] {
        let receiver = Config {
            water_mask,
            ..Config::default()
        };
        for observed_cell in [2, 4] {
            let sender = Config {
                width: 1280,
                height: 672,
                cell_size: 2,
                water_mask,
                ..Config::default()
            };
            let mut screen =
                ScreenTransfer::with_receiver_config_at(sender, &receiver, observed_cell, 83, 61);
            let capacity = screen.sender_grid.payload_capacity();
            assert_eq!(capacity, 107_514);
            assert!(capacity > receiver.grid().unwrap().payload_capacity() * 2);
            let payload = binary_payload(capacity * 2 + 19);
            let transfer = Transfer::new(
                TransferKind::File,
                "动态大画面.bin".into(),
                payload.clone(),
                capacity,
            )
            .unwrap();
            assert_eq!(transfer.total_frames(), 3);
            let mut session = screen.session(payload.len());
            assert_eq!(
                session
                    .accept(screen.capture_frame(transfer.info_frame()))
                    .unwrap(),
                ReceiveEvent::Metadata,
            );
            // Reconstruction remains ordered even if captures arrive out of
            // order, with every frame passing through the located pixels.
            for number in [3, 1, 2] {
                session
                    .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                    .unwrap();
            }
            assert!(session.is_complete());
            let output = tempfile::tempdir().unwrap();
            let path = session.save_file(output.path(), false).unwrap();
            assert_eq!(std::fs::read(path).unwrap(), payload);
        }
    }
}

#[test]
fn portrait_and_ultrawide_screens_locate_and_reconstruct_full_capacity_files() {
    for water_mask in [false, true] {
        let receiver = Config {
            water_mask,
            ..Config::default()
        };
        // Include shapes outside the former 1.2..=3.2 aspect-ratio limits,
        // with complete frames extending across a 1024px texture boundary.
        for (width, height) in [(320, 1920), (3840, 160), (1080, 1712), (3440, 872)] {
            let sender = Config {
                width,
                height,
                cell_size: 2,
                water_mask,
                ..Config::default()
            };
            let mut screen = ScreenTransfer::with_receiver_config_at(sender, &receiver, 2, 0, 29);
            let capacity = screen.sender_grid.payload_capacity();
            assert_eq!(capacity, (width as usize * height as usize / 8) - 6);
            let payload = binary_payload(capacity * 2 + 19);
            let transfer = Transfer::new(
                TransferKind::File,
                "全屏宽高比.bin".into(),
                payload.clone(),
                capacity,
            )
            .unwrap();
            let mut session = screen.session(payload.len());
            assert_eq!(
                session
                    .accept(screen.capture_frame(transfer.info_frame()))
                    .unwrap(),
                ReceiveEvent::Metadata
            );
            for number in [3, 1, 2] {
                session
                    .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                    .unwrap();
            }
            assert!(session.is_complete());
            assert!(session.missing_frames(100).is_empty());
            let output = tempfile::tempdir().unwrap();
            let path = session.save_file(output.path(), false).unwrap();
            assert_eq!(std::fs::read(path).unwrap(), payload);
        }
    }
}

#[test]
fn moving_and_resizing_calibration_relocates_and_restarts_with_the_new_frame_capacity() {
    for water_mask in [false, true] {
        let receiver = Config {
            water_mask,
            ..Config::default()
        };
        let initial_config = Config {
            width: 800,
            height: 448,
            cell_size: 2,
            water_mask,
            ..Config::default()
        };
        let mut screen =
            ScreenTransfer::with_receiver_config_at(initial_config.clone(), &receiver, 2, 41, 35);
        let initial_location = screen.location;
        let payload = binary_payload(158_074 * 2 + 17);
        let initial_transfer = Transfer::new(
            TransferKind::File,
            "窗口移动和缩放.bin".into(),
            payload.clone(),
            screen.sender_grid.payload_capacity(),
        )
        .unwrap();
        let mut partial = screen.session(payload.len());
        partial
            .accept(screen.capture_frame(initial_transfer.info_frame()))
            .unwrap();
        partial
            .accept(screen.capture_frame(initial_transfer.frame(1).unwrap()))
            .unwrap();
        assert_eq!(partial.received_count(), 1);
        assert!(!partial.is_complete());

        // Moving a window displays calibration again at the new origin.
        screen = ScreenTransfer::with_receiver_config_at(initial_config, &receiver, 2, 119, 87);
        assert_ne!(
            (screen.location.x, screen.location.y),
            (initial_location.x, initial_location.y)
        );
        assert_eq!(screen.location.grid, initial_location.grid);

        // Resizing changes both the captured rectangle and frame capacity.
        // A fresh calibration and metadata must govern the restarted file;
        // previous chunks were split according to a different capacity.
        let resized_config = Config {
            width: 1520,
            height: 832,
            cell_size: 2,
            water_mask,
            ..Config::default()
        };
        screen = ScreenTransfer::with_receiver_config_at(resized_config, &receiver, 2, 157, 113);
        assert_ne!(screen.location.grid, initial_location.grid);
        assert_eq!(screen.sender_grid.payload_capacity(), 158_074);
        let transfer = Transfer::new(
            TransferKind::File,
            "窗口移动和缩放.bin".into(),
            payload.clone(),
            screen.sender_grid.payload_capacity(),
        )
        .unwrap();
        assert_eq!(transfer.total_frames(), 3);
        assert_ne!(transfer.total_frames(), initial_transfer.total_frames());
        let mut session = screen.session(payload.len());
        assert_eq!(
            session
                .accept(screen.capture_frame(transfer.info_frame()))
                .unwrap(),
            ReceiveEvent::Metadata
        );
        for number in 1..=transfer.total_frames() {
            session
                .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                .unwrap();
        }
        assert!(session.is_complete());
        let output = tempfile::tempdir().unwrap();
        let path = session.save_file(output.path(), false).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), payload);
    }
}

fn place(screen: &mut RgbImage, panel: &RgbImage, x: u32, y: u32) {
    for (px, py, pixel) in panel.enumerate_pixels() {
        screen.put_pixel(x + px, y + py, *pixel);
    }
}

fn binary_payload(length: usize) -> Vec<u8> {
    // Includes every byte value, zeroes, and nibble extremes across cell rows.
    (0..length)
        .map(|index| ((index * 97 + index / 13) % 256) as u8)
        .collect()
}

#[test]
fn screen_to_file_roundtrip_in_both_modes_at_integer_cell_sizes() {
    for water_mask in [false, true] {
        let configurations = [
            (
                Config {
                    width: 80,
                    height: 48,
                    water_mask,
                    ..Config::default()
                },
                &[2, 4, 6, 8][..],
                114,
            ),
            (
                Config {
                    water_mask,
                    ..Config::default()
                },
                &[2][..],
                35_994,
            ),
        ];
        for (config, observed_cells, expected_capacity) in configurations {
            for &observed_cell in observed_cells {
                for tail in [1, 7, 113] {
                    // With the built-in 1440x800 configuration, a two-pixel cell
                    // locates a 720x400 panel while preserving all 360x200 cells.
                    // Both calibration and file frames pass through the same
                    // nearest-neighbor resize, including watermark quadrants.
                    let mut screen = ScreenTransfer::with_config(config.clone(), observed_cell);
                    let capacity = screen.sender_grid.payload_capacity();
                    assert_eq!(capacity, expected_capacity);
                    let payload = binary_payload(capacity * 2 + tail);
                    let transfer = Transfer::new(
                        TransferKind::File,
                        "跨屏 + transfer.bin".into(),
                        payload.clone(),
                        capacity,
                    )
                    .unwrap();
                    assert_eq!(transfer.total_frames(), 3);
                    let mut session = screen.session(payload.len());
                    assert_eq!(
                        session
                            .accept(screen.capture_frame(transfer.info_frame()))
                            .unwrap(),
                        ReceiveEvent::Metadata
                    );
                    for number in 1..=transfer.total_frames() {
                        session
                            .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                            .unwrap();
                    }
                    assert!(session.is_complete());
                    assert!(session.missing_frames(100).is_empty());
                    let output = tempfile::tempdir().unwrap();
                    let path = session.save_file(output.path(), false).unwrap();
                    assert_eq!(path.file_name().unwrap(), "跨屏 + transfer.bin");
                    assert_eq!(std::fs::read(path).unwrap(), payload);
                }
            }
        }
    }
}

#[test]
fn screen_to_file_recovers_missing_frames_without_losing_previous_chunks() {
    for water_mask in [false, true] {
        let mut screen = ScreenTransfer::new(water_mask, 6);
        let capacity = screen.sender_grid.payload_capacity();
        let payload = binary_payload(capacity * 4 + 13);
        let transfer = Transfer::new(
            TransferKind::File,
            "resend.bin".into(),
            payload.clone(),
            capacity,
        )
        .unwrap();
        let mut session = screen.session(payload.len());
        assert_eq!(
            session
                .accept(screen.capture_frame(transfer.frame(5).unwrap()))
                .unwrap(),
            ReceiveEvent::Ignored
        );
        session
            .accept(screen.capture_frame(transfer.info_frame()))
            .unwrap();
        for number in [5, 1, 3] {
            session
                .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                .unwrap();
        }
        assert_eq!(session.missing_frames(100), [2, 4]);
        assert!(!session.is_complete());
        let output = tempfile::tempdir().unwrap();
        assert!(session.save_file(output.path(), false).is_err());
        assert!(output.path().read_dir().unwrap().next().is_none());
        assert_eq!(
            session
                .accept(screen.capture_frame(transfer.info_frame()))
                .unwrap(),
            ReceiveEvent::Duplicate
        );
        assert_eq!(
            session
                .accept(screen.capture_frame(transfer.frame(1).unwrap()))
                .unwrap(),
            ReceiveEvent::Duplicate
        );
        assert_eq!(session.received_count(), 3);
        let specification = session
            .missing_frames(100)
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        for number in parse_resend(&specification, transfer.total_frames()).unwrap() {
            session
                .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                .unwrap();
        }
        assert!(session.is_complete());
        let path = session.save_file(output.path(), false).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), payload);
    }
}

#[test]
fn screen_to_text_preserves_unicode_and_line_breaks_when_scaled() {
    for water_mask in [false, true] {
        let mut screen = ScreenTransfer::new(water_mask, 6);
        let text = "Rust 重写😀\r\n第一行\n第二行\t + % &\n".repeat(11);
        let transfer = Transfer::new(
            TransferKind::Text,
            String::new(),
            text.as_bytes().to_vec(),
            screen.sender_grid.payload_capacity(),
        )
        .unwrap();
        let mut session = screen.session(text.len());
        session
            .accept(screen.capture_frame(transfer.info_frame()))
            .unwrap();
        for number in (1..=transfer.total_frames()).rev() {
            session
                .accept(screen.capture_frame(transfer.frame(number).unwrap()))
                .unwrap();
        }
        assert!(session.is_complete());
        assert_eq!(session.text().unwrap(), text);
    }
}
