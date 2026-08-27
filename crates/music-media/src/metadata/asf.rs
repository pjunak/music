use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use super::MetadataError;

const ASF_HEADER_GUID: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];
const FILE_PROPERTIES_GUID: [u8; 16] = [
    0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
];
const HEADER_PREFIX_BYTES: u64 = 30;
const OBJECT_HEADER_BYTES: u64 = 24;
const FILE_PROPERTIES_MINIMUM_BYTES: u64 = 64;
const MAX_HEADER_BYTES: u64 = 16 * 1024 * 1024;
const MAX_HEADER_OBJECTS: u32 = 16_384;

pub(super) fn duration(path: &Path) -> Result<Duration, MetadataError> {
    let mut file = File::open(path).map_err(|source| MetadataError::Io {
        action: "open ASF metadata source",
        source,
    })?;
    let file_size = file
        .metadata()
        .map_err(|source| MetadataError::Io {
            action: "inspect ASF metadata source",
            source,
        })?
        .len();

    let mut prefix = [0_u8; HEADER_PREFIX_BYTES as usize];
    file.read_exact(&mut prefix)
        .map_err(|source| MetadataError::Io {
            action: "read ASF header",
            source,
        })?;
    if prefix[..16] != ASF_HEADER_GUID {
        return Err(MetadataError::InvalidAsf(
            "header GUID does not identify ASF".to_owned(),
        ));
    }

    let header_size = read_u64(&prefix[16..24])?;
    let object_count = read_u32(&prefix[24..28])?;
    if !(HEADER_PREFIX_BYTES..=MAX_HEADER_BYTES).contains(&header_size) || header_size > file_size {
        return Err(MetadataError::InvalidAsf(
            "header size is outside the bounded file range".to_owned(),
        ));
    }
    if object_count > MAX_HEADER_OBJECTS {
        return Err(MetadataError::InvalidAsf(
            "header has too many objects".to_owned(),
        ));
    }

    let mut remaining = header_size - HEADER_PREFIX_BYTES;
    for _ in 0..object_count {
        if remaining < OBJECT_HEADER_BYTES {
            return Err(MetadataError::InvalidAsf(
                "object header exceeds the ASF header".to_owned(),
            ));
        }
        let mut object_header = [0_u8; OBJECT_HEADER_BYTES as usize];
        file.read_exact(&mut object_header)
            .map_err(|source| MetadataError::Io {
                action: "read ASF object header",
                source,
            })?;
        let object_size = read_u64(&object_header[16..24])?;
        if object_size < OBJECT_HEADER_BYTES || object_size > remaining {
            return Err(MetadataError::InvalidAsf(
                "object size exceeds the ASF header".to_owned(),
            ));
        }
        let payload_size = object_size - OBJECT_HEADER_BYTES;
        remaining -= object_size;

        if object_header[..16] == FILE_PROPERTIES_GUID {
            if payload_size < FILE_PROPERTIES_MINIMUM_BYTES {
                return Err(MetadataError::InvalidAsf(
                    "file-properties object is truncated".to_owned(),
                ));
            }
            let mut properties = [0_u8; FILE_PROPERTIES_MINIMUM_BYTES as usize];
            file.read_exact(&mut properties)
                .map_err(|source| MetadataError::Io {
                    action: "read ASF file properties",
                    source,
                })?;
            let play_duration_ticks = u128::from(read_u64(&properties[40..48])?);
            let preroll_millis = u128::from(read_u64(&properties[56..64])?);
            let play_nanos = play_duration_ticks.saturating_mul(100);
            let preroll_nanos = preroll_millis.saturating_mul(1_000_000);
            let duration_nanos = play_nanos.saturating_sub(preroll_nanos);
            let duration_nanos = u64::try_from(duration_nanos).map_err(|_| {
                MetadataError::InvalidAsf("duration exceeds the supported range".to_owned())
            })?;
            return Ok(Duration::from_nanos(duration_nanos));
        }

        let skip = i64::try_from(payload_size).map_err(|_| {
            MetadataError::InvalidAsf("object is too large to seek safely".to_owned())
        })?;
        file.seek(SeekFrom::Current(skip))
            .map_err(|source| MetadataError::Io {
                action: "skip ASF object",
                source,
            })?;
    }

    Err(MetadataError::InvalidAsf(
        "file-properties object is missing".to_owned(),
    ))
}

fn read_u64(bytes: &[u8]) -> Result<u64, MetadataError> {
    let value: [u8; 8] = bytes
        .try_into()
        .map_err(|_| MetadataError::InvalidAsf("expected an eight-byte integer".to_owned()))?;
    Ok(u64::from_le_bytes(value))
}

fn read_u32(bytes: &[u8]) -> Result<u32, MetadataError> {
    let value: [u8; 4] = bytes
        .try_into()
        .map_err(|_| MetadataError::InvalidAsf("expected a four-byte integer".to_owned()))?;
    Ok(u32::from_le_bytes(value))
}
