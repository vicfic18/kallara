use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek, SeekFrom, Write};
use thiserror::Error;

pub const HEADER_MAGIC: &[u8; 4] = b"KLR\x01";
pub const TRAILER_MAGIC: &[u8; 4] = b"RLK\x01";
pub const HEADER_SIZE: usize = 32;
pub const TRAILER_SIZE: usize = 28;
pub const FRAME_PLAINTEXT_SIZE: usize = 64 * 1024; // 64 KiB
pub const GCM_TAG_SIZE: usize = 16;
pub const ENCRYPTED_FRAME_SIZE: usize = FRAME_PLAINTEXT_SIZE + GCM_TAG_SIZE; // 65552 bytes

pub const FLAG_ENCRYPTED: u8 = 0x01;

#[derive(Error, Debug)]
pub enum FormatError {
    #[error("Invalid header magic, expected KLR\\x01")]
    InvalidHeaderMagic,
    #[error("Invalid trailer magic, expected RLK\\x01")]
    InvalidTrailerMagic,
    #[error("Invalid header checksum")]
    InvalidHeaderChecksum,
    #[error("Archive is too small to contain valid header and trailer")]
    ArchiveTooSmall,
    #[error("Invalid footer offset or size")]
    InvalidFooterBounds,
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("Cryptographic error: {0}")]
    Crypto(String),
    #[error("Compression error: {0}")]
    Compression(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub encrypted: bool,
    pub zstd_level: u8,
    pub salt: [u8; 16],
    pub base_nonce: [u8; 7],
}

impl Header {
    pub fn new(encrypted: bool, zstd_level: u8, salt: [u8; 16], base_nonce: [u8; 7]) -> Self {
        Self {
            encrypted,
            zstd_level,
            salt: if encrypted { salt } else { [0u8; 16] },
            base_nonce: if encrypted { base_nonce } else { [0u8; 7] },
        }
    }

    pub fn unencrypted(zstd_level: u8) -> Self {
        Self::new(false, zstd_level, [0u8; 16], [0u8; 7])
    }

    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(HEADER_MAGIC);
        buf[4] = if self.encrypted { FLAG_ENCRYPTED } else { 0 };
        buf[5] = self.zstd_level;
        buf[6..8].copy_from_slice(&[0, 0]); // Reserved
        buf[8..24].copy_from_slice(&self.salt);
        buf[24..31].copy_from_slice(&self.base_nonce);
        
        let checksum = compute_header_checksum(buf[0..31].try_into().unwrap());
        buf[31] = checksum;
        buf
    }

    pub fn from_bytes(bytes: &[u8; HEADER_SIZE]) -> Result<Self, FormatError> {
        if &bytes[0..4] != HEADER_MAGIC {
            return Err(FormatError::InvalidHeaderMagic);
        }

        let expected_checksum = compute_header_checksum(bytes[0..31].try_into().unwrap());
        if bytes[31] != expected_checksum {
            return Err(FormatError::InvalidHeaderChecksum);
        }

        let encrypted = (bytes[4] & FLAG_ENCRYPTED) != 0;
        let zstd_level = bytes[5];
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&bytes[8..24]);
        let mut base_nonce = [0u8; 7];
        base_nonce.copy_from_slice(&bytes[24..31]);

        Ok(Self {
            encrypted,
            zstd_level,
            salt,
            base_nonce,
        })
    }

    pub fn write_to<W: Write>(&self, mut writer: W) -> io::Result<()> {
        let bytes = self.to_bytes();
        writer.write_all(&bytes)
    }

    pub fn read_from<R: Read>(mut reader: R) -> Result<Self, FormatError> {
        let mut buf = [0u8; HEADER_SIZE];
        reader.read_exact(&mut buf)?;
        Self::from_bytes(&buf)
    }
}

pub fn compute_header_checksum(data: &[u8; 31]) -> u8 {
    data.iter().fold(0xA5, |acc, &b| acc.rotate_left(1) ^ b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trailer {
    pub footer_offset: u64,
    pub footer_compressed_size: u64,
    pub total_uncompressed_bytes: u64,
}

impl Trailer {
    pub fn new(footer_offset: u64, footer_compressed_size: u64, total_uncompressed_bytes: u64) -> Self {
        Self {
            footer_offset,
            footer_compressed_size,
            total_uncompressed_bytes,
        }
    }

    pub fn to_bytes(&self) -> [u8; TRAILER_SIZE] {
        let mut buf = [0u8; TRAILER_SIZE];
        buf[0..8].copy_from_slice(&self.footer_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.footer_compressed_size.to_le_bytes());
        buf[16..24].copy_from_slice(&self.total_uncompressed_bytes.to_le_bytes());
        buf[24..28].copy_from_slice(TRAILER_MAGIC);
        buf
    }

    pub fn from_bytes(bytes: &[u8; TRAILER_SIZE]) -> Result<Self, FormatError> {
        if &bytes[24..28] != TRAILER_MAGIC {
            return Err(FormatError::InvalidTrailerMagic);
        }

        let footer_offset = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let footer_compressed_size = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let total_uncompressed_bytes = u64::from_le_bytes(bytes[16..24].try_into().unwrap());

        Ok(Self {
            footer_offset,
            footer_compressed_size,
            total_uncompressed_bytes,
        })
    }

    pub fn write_to<W: Write>(&self, mut writer: W) -> io::Result<()> {
        let bytes = self.to_bytes();
        writer.write_all(&bytes)
    }

    pub fn read_from_end<R: Read + Seek>(mut reader: R) -> Result<Self, FormatError> {
        let len = reader.seek(SeekFrom::End(0))?;
        if len < (HEADER_SIZE + TRAILER_SIZE) as u64 {
            return Err(FormatError::ArchiveTooSmall);
        }

        reader.seek(SeekFrom::End(-(TRAILER_SIZE as i64)))?;
        let mut buf = [0u8; TRAILER_SIZE];
        reader.read_exact(&mut buf)?;
        let trailer = Self::from_bytes(&buf)?;

        if trailer.footer_offset < HEADER_SIZE as u64
            || trailer.footer_offset + trailer.footer_compressed_size > len - TRAILER_SIZE as u64
        {
            return Err(FormatError::InvalidFooterBounds);
        }

        Ok(trailer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexEntry {
    pub path: String,
    pub uncompressed_size: u64,
    pub tar_offset: u64,
    pub is_dir: bool,
    pub mode: u32,
    pub mtime: u64,
}
