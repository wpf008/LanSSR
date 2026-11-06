use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use crate::protocol::{Frame, Metadata, TransferKind};

#[derive(Debug, PartialEq, Eq)]
pub enum ReceiveEvent {
    Metadata,
    Data {
        number: u32,
        received: usize,
        total: u32,
    },
    Duplicate,
    Ignored,
}

/// One transfer; identical metadata during retransmission preserves received chunks.
pub struct ReceiveSession {
    metadata: Option<Metadata>,
    chunks: BTreeMap<u32, Vec<u8>>,
    received_bytes: usize,
    payload_capacity: usize,
    max_bytes: usize,
}

impl ReceiveSession {
    pub fn new(payload_capacity: usize, max_bytes: usize) -> Result<Self> {
        ensure!(
            payload_capacity > 0 && max_bytes > 0,
            "Receive limits must be positive"
        );
        Ok(Self {
            metadata: None,
            chunks: BTreeMap::new(),
            received_bytes: 0,
            payload_capacity,
            max_bytes,
        })
    }

    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }

    pub fn received_count(&self) -> usize {
        self.chunks.len()
    }

    /// Bytes in unique, validated data frames, maintained as chunks arrive.
    pub fn received_bytes(&self) -> usize {
        self.received_bytes
    }

    pub fn accept(&mut self, frame: Frame) -> Result<ReceiveEvent> {
        match frame {
            Frame::Info(metadata) => {
                ensure!(
                    metadata.total_frames <= crate::protocol::MAX_FRAMES,
                    "Too many frames"
                );
                let minimum_size = usize::try_from(metadata.total_frames.saturating_sub(1))?
                    .checked_mul(self.payload_capacity)
                    .context("Transfer size overflow")?;
                ensure!(
                    minimum_size < self.max_bytes,
                    "Transfer exceeds the receive size limit"
                );
                if metadata.kind == TransferKind::File {
                    validate_filename(&metadata.name)?;
                }
                if self.metadata.as_ref() == Some(&metadata) {
                    return Ok(ReceiveEvent::Duplicate);
                }
                self.chunks.clear();
                self.received_bytes = 0;
                self.metadata = Some(metadata);
                Ok(ReceiveEvent::Metadata)
            }
            Frame::Data { number, payload } => {
                let Some(metadata) = &self.metadata else {
                    return Ok(ReceiveEvent::Ignored);
                };
                ensure!(
                    (1..=metadata.total_frames).contains(&number),
                    "Frame {number} is outside this transfer"
                );
                ensure!(
                    payload.len() <= self.payload_capacity,
                    "Frame {number} exceeds the grid payload capacity"
                );
                if number < metadata.total_frames {
                    ensure!(
                        payload.len() == self.payload_capacity,
                        "Frame {number} is truncated"
                    );
                }
                ensure!(!payload.is_empty(), "Data frame {number} is empty");
                if self.chunks.contains_key(&number) {
                    return Ok(ReceiveEvent::Duplicate);
                }
                let size = self
                    .received_bytes
                    .checked_add(payload.len())
                    .context("Transfer size overflow")?;
                ensure!(
                    size <= self.max_bytes,
                    "Transfer exceeds the receive size limit"
                );
                self.chunks.insert(number, payload);
                self.received_bytes = size;
                Ok(ReceiveEvent::Data {
                    number,
                    received: self.chunks.len(),
                    total: metadata.total_frames,
                })
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        self.metadata
            .as_ref()
            .is_some_and(|metadata| self.chunks.len() == metadata.total_frames as usize)
    }

    pub fn missing_frames(&self, limit: usize) -> Vec<u32> {
        match &self.metadata {
            Some(metadata) => (1..=metadata.total_frames)
                .filter(|number| !self.chunks.contains_key(number))
                .take(limit)
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn text(&self) -> Result<String> {
        ensure!(self.is_complete(), "Transfer is incomplete");
        ensure!(
            self.metadata.as_ref().unwrap().kind == TransferKind::Text,
            "Transfer is not text"
        );
        let bytes: Vec<u8> = self
            .chunks
            .values()
            .flat_map(|chunk| chunk.iter().copied())
            .collect();
        String::from_utf8(bytes).context("Received text is not valid UTF-8")
    }

    /// Write in order to a temporary file, then atomically publish the completed result.
    pub fn save_file(&self, output_dir: &Path, overwrite: bool) -> Result<PathBuf> {
        ensure!(self.is_complete(), "Transfer is incomplete");
        let metadata = self.metadata.as_ref().unwrap();
        ensure!(
            metadata.kind == TransferKind::File,
            "Transfer is not a file"
        );
        validate_filename(&metadata.name)?;
        let destination = output_dir.join(&metadata.name);
        let mut temporary = tempfile::NamedTempFile::new_in(output_dir)
            .with_context(|| format!("Cannot create file in {}", output_dir.display()))?;
        for chunk in self.chunks.values() {
            temporary.write_all(chunk)?;
        }
        temporary.as_file().sync_all()?;
        if overwrite {
            temporary
                .persist(&destination)
                .map_err(|error| error.error)?;
        } else {
            temporary
                .persist_noclobber(&destination)
                .map_err(|error| error.error)
                .with_context(|| {
                    format!(
                        "Cannot save {} (use --overwrite to replace an existing file)",
                        destination.display()
                    )
                })?;
        }
        Ok(destination)
    }
}

fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        bail!("Invalid received filename: {name:?}");
    }
    ensure!(
        Path::new(name).file_name().and_then(|value| value.to_str()) == Some(name),
        "Received filename must be a single path component"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Transfer;

    #[test]
    fn out_of_order_resend_and_atomic_save() {
        let transfer = Transfer::new(
            TransferKind::File,
            "你好 + test.bin".into(),
            (0..23).collect(),
            8,
        )
        .unwrap();
        let mut session = ReceiveSession::new(8, 100).unwrap();
        assert_eq!(
            session.accept(transfer.frame(1).unwrap()).unwrap(),
            ReceiveEvent::Ignored
        );
        session.accept(transfer.info_frame()).unwrap();
        assert_eq!(session.received_bytes(), 0);
        session.accept(transfer.frame(3).unwrap()).unwrap();
        session.accept(transfer.frame(1).unwrap()).unwrap();
        assert_eq!(session.received_bytes(), 15);
        assert_eq!(session.missing_frames(100), vec![2]);
        assert_eq!(
            session.accept(transfer.info_frame()).unwrap(),
            ReceiveEvent::Duplicate
        );
        assert_eq!(
            session.accept(transfer.frame(1).unwrap()).unwrap(),
            ReceiveEvent::Duplicate
        );
        assert_eq!(session.received_bytes(), 15);
        assert!(!session.is_complete());
        session.accept(transfer.frame(2).unwrap()).unwrap();
        assert_eq!(session.received_bytes(), 23);
        let dir = tempfile::tempdir().unwrap();
        let path = session.save_file(dir.path(), false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), (0..23).collect::<Vec<u8>>());
        assert!(session.save_file(dir.path(), false).is_err());
        assert_eq!(std::fs::read(&path).unwrap().len(), 23);
        session.save_file(dir.path(), true).unwrap();
    }

    #[test]
    fn handles_empty_files_and_unicode_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = ReceiveSession::new(8, 100).unwrap();
        let empty = Transfer::new(TransferKind::File, "empty".into(), vec![], 8).unwrap();
        session.accept(empty.info_frame()).unwrap();
        assert!(session.is_complete());
        assert_eq!(
            std::fs::metadata(session.save_file(dir.path(), false).unwrap())
                .unwrap()
                .len(),
            0
        );
        let text = "Rust 重写 😀\n";
        let transfer =
            Transfer::new(TransferKind::Text, "".into(), text.as_bytes().to_vec(), 8).unwrap();
        session.accept(transfer.info_frame()).unwrap();
        for number in 1..=transfer.total_frames() {
            session.accept(transfer.frame(number).unwrap()).unwrap();
        }
        assert_eq!(session.text().unwrap(), text);
    }

    #[test]
    fn bounds_frames_paths_and_memory() {
        let mut session = ReceiveSession::new(8, 16).unwrap();
        let metadata = Metadata {
            kind: TransferKind::File,
            total_frames: 2,
            name: "../escape".into(),
        };
        assert!(session.accept(Frame::Info(metadata)).is_err());
        session
            .accept(Frame::Info(Metadata {
                kind: TransferKind::File,
                total_frames: 2,
                name: "ok".into(),
            }))
            .unwrap();
        assert!(
            session
                .accept(Frame::Data {
                    number: 3,
                    payload: vec![0; 8]
                })
                .is_err()
        );
        assert!(
            session
                .accept(Frame::Data {
                    number: 1,
                    payload: vec![0; 7]
                })
                .is_err()
        );
        assert!(
            session
                .accept(Frame::Data {
                    number: 1,
                    payload: vec![0; 9]
                })
                .is_err()
        );
        assert!(session.save_file(Path::new("."), false).is_err());
        assert!(
            session
                .accept(Frame::Info(Metadata {
                    kind: TransferKind::Text,
                    total_frames: 4,
                    name: "".into()
                }))
                .is_err()
        );
    }
}
