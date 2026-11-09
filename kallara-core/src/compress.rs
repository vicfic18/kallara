use crate::format::FormatError;

pub const DEFAULT_ZSTD_LEVEL: u8 = 3;

/// Compresses data using Zstd at the given compression level.
pub fn compress_zstd(data: &[u8], level: u8) -> Result<Vec<u8>, FormatError> {
    zstd::encode_all(data, level as i32)
        .map_err(|e| FormatError::Compression(format!("Zstd compression failed: {}", e)))
}

/// Decompresses data using Zstd.
pub fn decompress_zstd(data: &[u8]) -> Result<Vec<u8>, FormatError> {
    zstd::decode_all(data)
        .map_err(|e| FormatError::Compression(format!("Zstd decompression failed: {}", e)))
}

/// Computes a fast sample-based estimate of compressed size.
/// Samples up to 256KB from the provided file paths/bytes and calculates estimated ratio.
pub fn estimate_compressed_size(total_uncompressed: u64, sample_data: &[u8], level: u8) -> u64 {
    if total_uncompressed == 0 {
        return 0;
    }

    if sample_data.is_empty() {
        // Fallback default ~40% ratio for typical code/data archives
        return (total_uncompressed as f64 * 0.40) as u64;
    }

    // Limit sample size to 256 KB for < 1ms execution
    let sample_slice = if sample_data.len() > 256 * 1024 {
        &sample_data[..256 * 1024]
    } else {
        sample_data
    };

    match zstd::encode_all(sample_slice, level as i32) {
        Ok(compressed) => {
            let ratio = compressed.len() as f64 / sample_slice.len() as f64;
            let estimated = (total_uncompressed as f64 * ratio) as u64;
            // Bound estimate sensibly
            estimated.max(32)
        }
        Err(_) => (total_uncompressed as f64 * 0.40) as u64,
    }
}

/// Buffer helper to slice compressed streams into 64KB frames.
pub struct FrameBuffer {
    buffer: Vec<u8>,
    pub frame_size: usize,
}

impl FrameBuffer {
    pub fn new(frame_size: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(frame_size * 2),
            frame_size,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    pub fn pop_frame(&mut self) -> Option<Vec<u8>> {
        if self.buffer.len() >= self.frame_size {
            let frame = self.buffer.drain(..self.frame_size).collect();
            Some(frame)
        } else {
            None
        }
    }

    pub fn drain_remaining(&mut self) -> Option<Vec<u8>> {
        if !self.buffer.is_empty() {
            let frame = self.buffer.split_off(0);
            Some(frame)
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}
