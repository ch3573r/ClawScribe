//! Length-delimited JSON and little-endian PCM. No diagnostics include payloads.
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

pub const MAX_SECTION_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Request {
    Load {
        model_dir: PathBuf,
        precision_suffix: Option<String>,
        use_directml: bool,
    },
    Transcribe,
    Shutdown,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Response {
    Loaded {
        directml_registered: bool,
    },
    Transcript {
        text: String,
        timestamps: Vec<f32>,
        tokens: Vec<String>,
    },
    Error(String),
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid transcription worker frame",
    )
}

struct JsonBuffer(Vec<u8>);
impl Write for JsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_SECTION_BYTES.saturating_sub(self.0.len()) {
            return Err(invalid());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn write_frame<T: Serialize>(
    writer: &mut impl Write,
    message: &T,
    samples: &[f32],
) -> io::Result<()> {
    let pcm_bytes = samples.len().checked_mul(4).ok_or_else(invalid)?;
    if pcm_bytes > MAX_SECTION_BYTES {
        return Err(invalid());
    }
    let mut json = JsonBuffer(Vec::new());
    serde_json::to_writer(&mut json, message).map_err(|_| invalid())?;
    writer.write_all(&(json.0.len() as u32).to_le_bytes())?;
    writer.write_all(&json.0)?;
    writer.write_all(&(pcm_bytes as u32).to_le_bytes())?;
    let mut buffer = [0u8; 8192];
    for chunk in samples.chunks(buffer.len() / 4) {
        for (sample, bytes) in chunk.iter().zip(buffer.chunks_exact_mut(4)) {
            bytes.copy_from_slice(&sample.to_le_bytes());
        }
        writer.write_all(&buffer[..chunk.len() * 4])?;
    }
    writer.flush()
}

/// `None` is EOF between frames; EOF inside a frame is an error.
pub fn read_frame<T: DeserializeOwned>(
    reader: &mut impl Read,
) -> io::Result<Option<(T, Vec<f32>)>> {
    let mut length = [0u8; 4];
    loop {
        match reader.read(&mut length[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut length[1..])?;
    let json_bytes = u32::from_le_bytes(length) as usize;
    if json_bytes > MAX_SECTION_BYTES {
        return Err(invalid());
    }
    let mut json = vec![0u8; json_bytes];
    reader.read_exact(&mut json)?;
    let message = serde_json::from_slice(&json).map_err(|_| invalid())?;
    reader.read_exact(&mut length)?;
    let pcm_bytes = u32::from_le_bytes(length) as usize;
    if pcm_bytes > MAX_SECTION_BYTES || pcm_bytes % 4 != 0 {
        return Err(invalid());
    }
    let mut samples = Vec::with_capacity(pcm_bytes / 4);
    let mut buffer = [0u8; 8192];
    let mut remaining = pcm_bytes;
    while remaining > 0 {
        let count = remaining.min(buffer.len());
        reader.read_exact(&mut buffer[..count])?;
        for bytes in buffer[..count].chunks_exact(4) {
            samples.push(f32::from_le_bytes(bytes.try_into().map_err(|_| invalid())?));
        }
        remaining -= count;
    }
    Ok(Some((message, samples)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn request_and_response_round_trip() {
        let requests = [
            Request::Load {
                model_dir: "models/parakeet".into(),
                precision_suffix: Some("int8".into()),
                use_directml: true,
            },
            Request::Transcribe,
            Request::Shutdown,
        ];
        for request in requests {
            let samples = if matches!(request, Request::Transcribe) {
                vec![0.0, -0.25, 0.5]
            } else {
                vec![]
            };
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &request, &samples).unwrap();
            let mut reader = Cursor::new(bytes);
            assert_eq!(
                read_frame::<Request>(&mut reader).unwrap(),
                Some((request, samples))
            );
            assert!(read_frame::<Request>(&mut reader).unwrap().is_none());
        }
        for response in [
            Response::Loaded {
                directml_registered: false,
            },
            Response::Transcript {
                text: "test".into(),
                timestamps: vec![0.1],
                tokens: vec!["test".into()],
            },
            Response::Error("Engine unavailable".into()),
        ] {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &response, &[]).unwrap();
            assert_eq!(
                read_frame::<Response>(&mut Cursor::new(bytes)).unwrap(),
                Some((response, vec![]))
            );
        }
    }

    #[test]
    fn every_truncated_section_is_rejected() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Transcribe, &[0.5]).unwrap();
        for length in 1..bytes.len() {
            assert!(read_frame::<Request>(&mut &bytes[..length]).is_err());
        }
        assert!(read_frame::<Request>(&mut &[][..]).unwrap().is_none());
    }

    #[test]
    fn oversized_json_and_pcm_are_rejected_before_allocation() {
        let oversized = (MAX_SECTION_BYTES as u32 + 1).to_le_bytes();
        assert_eq!(
            read_frame::<Request>(&mut &oversized[..])
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        let json = serde_json::to_vec(&Request::Transcribe).unwrap();
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json);
        bytes.extend(oversized);
        assert_eq!(
            read_frame::<Request>(&mut &bytes[..]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn zero_length_pcm_is_valid() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Transcribe, &[]).unwrap();
        assert_eq!(
            read_frame::<Request>(&mut &bytes[..]).unwrap(),
            Some((Request::Transcribe, vec![]))
        );
    }

    #[test]
    fn wire_lengths_and_samples_are_little_endian() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Transcribe, &[0.5]).unwrap();
        let mut expected = vec![12, 0, 0, 0];
        expected.extend_from_slice(b"\"Transcribe\"");
        expected.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 63]);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn malformed_json_and_unaligned_pcm_are_rejected() {
        assert!(read_frame::<Request>(&mut &[1, 0, 0, 0, b'{'][..]).is_err());
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Transcribe, &[]).unwrap();
        let end = bytes.len();
        bytes[end - 4..].copy_from_slice(&1u32.to_le_bytes());
        bytes.push(0);
        assert_eq!(
            read_frame::<Request>(&mut &bytes[..]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
