//! The original Java wire format, independent of image and desktop handling.

use anyhow::{Context, Result, bail, ensure};

/// The largest frame number accepted by the Java receiver.
pub const MAX_FRAMES: u32 = 986_894;
const HEADER_LEN: usize = 5;
const MAX_RESEND_INPUT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferKind {
    File,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub kind: TransferKind,
    pub total_frames: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Info(Metadata),
    Data { number: u32, payload: Vec<u8> },
}

impl Frame {
    /// Metadata uses Java's form URL encoding; data uses the MD5 digest's last
    /// two bytes. The checksum is retained for interoperability, not security.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Info(metadata) => {
                ensure!(metadata.total_frames <= MAX_FRAMES, "too many frames");
                let name = encode_name(&metadata.name);
                let mut bytes = Vec::with_capacity(HEADER_LEN + name.len());
                bytes.push(match metadata.kind {
                    TransferKind::File => b'f',
                    TransferKind::Text => b't',
                });
                bytes.extend_from_slice(&metadata.total_frames.to_be_bytes());
                bytes.extend_from_slice(name.as_bytes());
                Ok(bytes)
            }
            Self::Data { number, payload } => {
                ensure!(
                    (1..=MAX_FRAMES).contains(number),
                    "invalid frame number {number}"
                );
                let number_bytes = number.to_be_bytes();
                let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
                bytes.extend_from_slice(&number_bytes[1..]);
                bytes.extend_from_slice(&md5::compute(payload).0[14..]);
                bytes.extend_from_slice(payload);
                Ok(bytes)
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= HEADER_LEN,
            "frame is shorter than its header"
        );
        if bytes[0] == b'f' || bytes[0] == b't' {
            let total_frames = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
            ensure!(
                total_frames <= MAX_FRAMES,
                "metadata declares too many frames"
            );
            return Ok(Self::Info(Metadata {
                kind: if bytes[0] == b'f' {
                    TransferKind::File
                } else {
                    TransferKind::Text
                },
                total_frames,
                name: decode_name(&bytes[HEADER_LEN..])?,
            }));
        }

        let number = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        ensure!(
            (1..=MAX_FRAMES).contains(&number),
            "invalid frame number {number}"
        );
        let payload = &bytes[HEADER_LEN..];
        let digest = md5::compute(payload);
        ensure!(
            bytes[3..5] == digest.0[14..],
            "frame {number} failed its MD5 checksum"
        );
        Ok(Self::Data {
            number,
            payload: payload.to_vec(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Transfer {
    pub metadata: Metadata,
    payload: Vec<u8>,
    payload_size: usize,
}

impl Transfer {
    pub fn new(
        kind: TransferKind,
        name: String,
        payload: Vec<u8>,
        payload_size: usize,
    ) -> Result<Self> {
        ensure!(payload_size > 0, "frame payload capacity must be positive");
        let total = payload.len().div_ceil(payload_size);
        ensure!(
            total <= MAX_FRAMES as usize,
            "transfer needs {total} frames; maximum is {MAX_FRAMES}"
        );
        Ok(Self {
            metadata: Metadata {
                kind,
                total_frames: total as u32,
                name,
            },
            payload,
            payload_size,
        })
    }

    pub fn info_frame(&self) -> Frame {
        Frame::Info(self.metadata.clone())
    }

    pub fn frame(&self, number: u32) -> Result<Frame> {
        // Check the payload as well as metadata because callers can update the
        // public metadata without changing the underlying chunks.
        ensure!(
            number > 0 && number <= self.total_frames(),
            "frame {number} is outside this transfer"
        );
        let start = (number as usize - 1)
            .checked_mul(self.payload_size)
            .context("frame offset overflow")?;
        ensure!(start < self.payload.len(), "frame {number} has no payload");
        let end = start
            .saturating_add(self.payload_size)
            .min(self.payload.len());
        Ok(Frame::Data {
            number,
            payload: self.payload[start..end].to_vec(),
        })
    }

    pub fn total_frames(&self) -> u32 {
        self.metadata.total_frames
    }

    pub fn byte_len(&self) -> usize {
        self.payload.len()
    }

    /// Repartition already-read bytes when the visible board capacity changes.
    pub fn with_payload_capacity(self, payload_size: usize) -> Result<Self> {
        Self::new(
            self.metadata.kind,
            self.metadata.name,
            self.payload,
            payload_size,
        )
    }
}

/// Expand an inclusive resend specification. Whitespace-only means every
/// frame; malformed, reversed, or out-of-bounds ranges are rejected.
pub fn parse_resend(spec: &str, total: u32) -> Result<Vec<u32>> {
    ensure!(total <= MAX_FRAMES, "too many frames");
    ensure!(
        spec.len() <= MAX_RESEND_INPUT,
        "resend specification is too long"
    );
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok((1..=total).collect());
    }
    ensure!(
        spec.bytes()
            .all(|b| b.is_ascii_digit() || b == b',' || b == b'-'),
        "resend accepts only numbers, commas, and hyphens"
    );

    // A bounded bitmap avoids repeated allocation for overlapping ranges.
    let mut selected = vec![false; total as usize + 1];
    for item in spec.split(',') {
        ensure!(!item.is_empty(), "empty entry in resend specification");
        let (start, end) = match item.split_once('-') {
            Some((start, end)) => {
                ensure!(
                    !start.is_empty() && !end.is_empty() && !end.contains('-'),
                    "invalid resend range {item}"
                );
                (
                    parse_frame_number(start, total)?,
                    parse_frame_number(end, total)?,
                )
            }
            None => {
                let number = parse_frame_number(item, total)?;
                (number, number)
            }
        };
        ensure!(start <= end, "reversed resend range {item}");
        selected[start as usize..=end as usize].fill(true);
    }
    Ok(selected
        .into_iter()
        .enumerate()
        .filter_map(|(number, present)| present.then_some(number as u32))
        .collect())
}

fn parse_frame_number(text: &str, total: u32) -> Result<u32> {
    let number = text
        .parse::<u32>()
        .with_context(|| format!("invalid frame number {text}"))?;
    ensure!(
        number > 0 && number <= total,
        "frame {number} must be in 1..={total}"
    );
    Ok(number)
}

fn encode_name(name: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in name.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'*' | b'_' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 15) as usize] as char);
            }
        }
    }
    encoded
}

fn decode_name(encoded: &[u8]) -> Result<String> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        match encoded[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                ensure!(index + 2 < encoded.len(), "incomplete URL escape in name");
                let high = hex_value(encoded[index + 1])?;
                let low = hex_value(encoded[index + 2])?;
                decoded.push(high << 4 | low);
                index += 2;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).context("name is not valid UTF-8")
}

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => bail!("invalid URL escape in name"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_data_wire_vector_and_checksum() {
        // MD5("abc") = 900150983cd24fb0d6963f7d28e17f72.
        let frame = Frame::Data {
            number: 0x010203,
            payload: b"abc".to_vec(),
        };
        let expected = [1, 2, 3, 0x7f, 0x72, b'a', b'b', b'c'];
        assert_eq!(frame.encode().unwrap(), expected);
        assert_eq!(Frame::decode(&expected).unwrap(), frame);
        let mut corrupt = expected;
        corrupt[6] ^= 1;
        assert!(Frame::decode(&corrupt).is_err());
    }

    #[test]
    fn java_metadata_url_encoding_vector() {
        let frame = Frame::Info(Metadata {
            kind: TransferKind::File,
            total_frames: 258,
            name: "中 文+~!*_.-.txt".into(),
        });
        let mut expected = vec![b'f', 0, 0, 1, 2];
        expected.extend_from_slice(b"%E4%B8%AD+%E6%96%87%2B%7E%21*_.-.txt");
        assert_eq!(frame.encode().unwrap(), expected);
        assert_eq!(Frame::decode(&expected).unwrap(), frame);
        assert_eq!(
            Frame::decode(b"t\0\0\0\0").unwrap(),
            Frame::Info(Metadata {
                kind: TransferKind::Text,
                total_frames: 0,
                name: String::new()
            })
        );
    }

    #[test]
    fn rejects_invalid_frames_and_names() {
        for bytes in [
            b"".as_slice(),
            &[0, 0, 0, 0, 0],
            b"f\0\0\0\x01%",
            b"f\0\0\0\x01%GG",
            b"f\0\0\0\x01%FF",
        ] {
            assert!(Frame::decode(bytes).is_err(), "accepted {bytes:?}");
        }
        for number in [0, MAX_FRAMES + 1, u32::MAX] {
            assert!(
                Frame::Data {
                    number,
                    payload: vec![]
                }
                .encode()
                .is_err()
            );
        }
        let max = Frame::Data {
            number: MAX_FRAMES,
            payload: vec![],
        };
        assert_eq!(Frame::decode(&max.encode().unwrap()).unwrap(), max);
        assert!(
            Frame::Info(Metadata {
                kind: TransferKind::File,
                total_frames: MAX_FRAMES + 1,
                name: String::new()
            })
            .encode()
            .is_err()
        );
    }

    #[test]
    fn splits_without_losing_final_bytes_and_handles_empty_transfers() {
        let transfer =
            Transfer::new(TransferKind::Text, String::new(), b"abcdefg".to_vec(), 3).unwrap();
        assert_eq!(transfer.total_frames(), 3);
        assert_eq!(transfer.byte_len(), 7);
        let mut bytes = Vec::new();
        for number in 1..=3 {
            match transfer.frame(number).unwrap() {
                Frame::Data { payload, .. } => bytes.extend(payload),
                _ => unreachable!(),
            }
        }
        assert_eq!(bytes, b"abcdefg");
        assert!(transfer.frame(0).is_err());
        assert!(transfer.frame(4).is_err());
        let empty = Transfer::new(TransferKind::File, "empty".into(), vec![], 1).unwrap();
        assert_eq!(empty.total_frames(), 0);
        assert!(empty.frame(1).is_err());
        assert!(Transfer::new(TransferKind::Text, String::new(), vec![], 0).is_err());
        assert!(
            Transfer::new(
                TransferKind::Text,
                String::new(),
                vec![0; MAX_FRAMES as usize + 1],
                1
            )
            .is_err()
        );
    }

    #[test]
    fn resend_is_sorted_deduplicated_and_bounded() {
        assert_eq!(parse_resend("8,3-5,1,4", 10).unwrap(), [1, 3, 4, 5, 8]);
        assert_eq!(parse_resend(" \n", 3).unwrap(), [1, 2, 3]);
        assert!(parse_resend("", 0).unwrap().is_empty());
        for spec in [
            "0",
            "11",
            "5-3",
            "-1",
            "1-",
            "1-2-3",
            "1,,2",
            "1,",
            ",1",
            "1, 2",
            "a",
            "4294967296",
        ] {
            assert!(parse_resend(spec, 10).is_err(), "accepted {spec}");
        }
        assert!(parse_resend("1", 0).is_err());
        assert!(parse_resend("", MAX_FRAMES + 1).is_err());
        assert!(parse_resend(&"1".repeat(MAX_RESEND_INPUT + 1), 10).is_err());
    }
}
