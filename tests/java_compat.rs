//! Run explicitly with a JDK installed:
//! cargo test --no-default-features --test java_compat -- --ignored

use anyhow::{Context, Result, ensure};
use imgbridge::grid::Grid;
use imgbridge::protocol::{Frame, Metadata, TransferKind};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
#[ignore = "requires javac and java; compiles and runs the original Java implementation"]
fn original_java_and_rust_interoperate_in_both_image_modes() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temporary = tempfile::tempdir()?;
    let classes = temporary.path().join("classes");
    fs::create_dir(&classes)?;
    let mut sources = Vec::new();
    java_sources(&root.join("src/main/java"), &mut sources)?;
    sources.push(root.join("tests/fixtures/Compat.java"));
    let mut compiler = Command::new("javac");
    compiler
        .args(["-encoding", "UTF-8", "-d"])
        .arg(&classes)
        .args(&sources);
    run(compiler).context("compile original Java implementation and interoperability fixture")?;

    let payload: Vec<u8> = (0..=255).collect();
    let filename = "中 文+~!*_.-.txt";
    for water_mask in [false, true] {
        let grid = Grid::new(128, 128, 4, water_mask)?;
        for kind in [TransferKind::File, TransferKind::Text] {
            let kind_label = match kind {
                TransferKind::File => "file",
                TransferKind::Text => "text",
            };
            let directory = temporary.path().join(format!("{water_mask}-{kind_label}"));
            fs::create_dir(&directory)?;
            let source = directory.join(filename);
            fs::write(&source, &payload)?;
            let metadata = Metadata {
                kind: kind.clone(),
                total_frames: 1,
                name: if kind == TransferKind::File {
                    filename.into()
                } else {
                    String::new()
                },
            };
            let frames = [
                Frame::Info(metadata.clone()),
                Frame::Data {
                    number: 1,
                    payload: payload.clone(),
                },
            ];

            // Actual Java SendThread constructs its metadata and MD5 header;
            // PicTransmitter.drawBytes draws the output captured by the fixture.
            let mut sender = java(&classes, "encode", water_mask, &directory);
            sender.arg(&source).arg(kind_label);
            run(sender)?;
            for (index, expected) in frames.iter().enumerate() {
                let java_image =
                    image::open(directory.join(format!("frame-{index}.png")))?.to_rgb8();
                let java_bytes = grid.decode(&java_image)?;
                ensure!(
                    Frame::decode(&java_bytes)? == *expected,
                    "Rust rejected Java {kind_label} frame {index}, watermark={water_mask}"
                );
                let rust_image = grid.encode(&expected.encode()?)?;
                ensure!(
                    rust_image == java_image,
                    "Java and Rust pixel output differs for frame {index}, watermark={water_mask}"
                );
                // Replace captured Java output with independent Rust output.
                rust_image.save(directory.join(format!("frame-{index}.png")))?;
            }

            // Actual Java ImgUtil.parseBytes and RPaserThread must accept the
            // Rust images, decoded metadata, frame number, and MD5 checksum.
            run(java(&classes, "decode", water_mask, &directory))?;
            for (index, frame) in frames.iter().enumerate() {
                ensure!(
                    fs::read(directory.join(format!("frame-{index}.bin")))? == frame.encode()?,
                    "Java decoded incorrect Rust bytes in frame {index}, watermark={water_mask}"
                );
            }
            ensure!(
                fs::read(directory.join("received.bin"))? == payload,
                "original Java receiver reconstructed an incorrect payload"
            );
            ensure!(
                fs::read_to_string(directory.join("metadata.txt"))?
                    == format!("{kind_label}\n1\n{}", metadata.name),
                "original Java receiver decoded incorrect metadata"
            );
        }
    }
    Ok(())
}

fn java(classes: &Path, operation: &str, water_mask: bool, directory: &Path) -> Command {
    let mut command = Command::new("java");
    command
        .arg("-Djava.awt.headless=true")
        .arg("-cp")
        .arg(classes)
        .arg("Compat")
        .arg(operation)
        .arg(water_mask.to_string())
        .arg(directory);
    command
}

fn run(mut command: Command) -> Result<()> {
    let output = command.output().with_context(|| {
        format!("could not execute {command:?}; install a JDK for this optional test")
    })?;
    ensure!(
        output.status.success(),
        "command {command:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn java_sources(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in directory.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            java_sources(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "java")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(())
}
