use crate::compress::{compress_zstd, decompress_zstd, FrameBuffer};
use crate::crypto::{
    decrypt_footer, decrypt_frame, derive_key, encrypt_footer, encrypt_frame, generate_crypto_params,
    KEY_SIZE,
};
use crate::format::{
    FormatError, Header, IndexEntry, Trailer, FRAME_PLAINTEXT_SIZE,
    HEADER_SIZE,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ArchiveProgress {
    pub bytes_processed: u64,
    pub total_bytes: u64,
    pub current_speed_bps: f64, // bytes per second
    pub phase: ProgressPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressPhase {
    Staging,
    Archiving,
    Extracting,
    Completed,
}

struct CountingWriter<W: Write, F: FnMut(usize)> {
    inner: W,
    on_write: F,
}

impl<W: Write, F: FnMut(usize)> Write for CountingWriter<W, F> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        (self.on_write)(n);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct ProgressReader<R: Read, F: FnMut(usize)> {
    inner: R,
    on_read: F,
}

impl<R: Read, F: FnMut(usize)> Read for ProgressReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            (self.on_read)(n);
        }
        Ok(n)
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveMetadata {
    pub header: Header,
    pub trailer: Trailer,
    pub entries: Vec<IndexEntry>,
    pub total_uncompressed_bytes: u64,
    pub total_compressed_bytes: u64,
    pub file_count: usize,
    pub dir_count: usize,
}

/// O(1) Inspection of `.klr` archive (< 5ms).
/// Seeks directly to `SeekFrom::End(-28)`, reads Trailer, seeks to footer, decrypts/decompresses IndexEntry table.
pub fn inspect_archive<P: AsRef<Path>>(
    path: P,
    passphrase: Option<&str>,
) -> Result<ArchiveMetadata, FormatError> {
    let mut file = File::open(path)?;
    let header = Header::read_from(&mut file)?;
    let trailer = Trailer::read_from_end(&mut file)?;

    // Seek to footer offset
    file.seek(SeekFrom::Start(trailer.footer_offset))?;
    let mut footer_bytes = vec![0u8; trailer.footer_compressed_size as usize];
    file.read_exact(&mut footer_bytes)?;

    let decompressed_footer = if header.encrypted {
        let pass = passphrase.unwrap_or("");
        if pass.is_empty() {
            return Err(FormatError::Crypto("Passphrase required to unlock archive".to_string()));
        }
        let key = derive_key(pass, &header.salt)?;
        let decrypted = decrypt_footer(&key, &header.base_nonce, &footer_bytes)?;
        decompress_zstd(&decrypted)?
    } else {
        decompress_zstd(&footer_bytes)?
    };

    let entries: Vec<IndexEntry> = bincode::deserialize(&decompressed_footer)?;

    let mut file_count = 0;
    let mut dir_count = 0;
    for entry in &entries {
        if entry.is_dir {
            dir_count += 1;
        } else {
            file_count += 1;
        }
    }

    let file_len = file.metadata()?.len();

    Ok(ArchiveMetadata {
        header,
        trailer,
        entries,
        total_uncompressed_bytes: trailer.total_uncompressed_bytes,
        total_compressed_bytes: file_len,
        file_count,
        dir_count,
    })
}

/// Item to stage for archiving: physical path on disk + relative path in archive.
#[derive(Debug, Clone)]
pub struct StagedItem {
    pub disk_path: PathBuf,
    pub archive_path: String,
}

/// Collects files and directories recursively from a given disk path.
pub fn collect_staged_items<P: AsRef<Path>>(base_path: P) -> io::Result<Vec<StagedItem>> {
    let base_path = base_path.as_ref();
    let mut items = Vec::new();

    if base_path.is_file() {
        let name = base_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        items.push(StagedItem {
            disk_path: base_path.to_path_buf(),
            archive_path: name,
        });
        return Ok(items);
    }

    let walker = jwalk::WalkDir::new(base_path).sort(true);
    for entry in walker {
        let entry = entry.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let full_path = entry.path();
        if full_path == base_path {
            continue;
        }
        let rel_path = full_path
            .strip_prefix(base_path)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
            .to_string_lossy()
            .to_string();

        items.push(StagedItem {
            disk_path: full_path,
            archive_path: rel_path,
        });
    }

    Ok(items)
}

/// Progress callback type.
pub type ProgressCallback = Arc<dyn Fn(ArchiveProgress) + Send + Sync>;

/// Creates a new `.klr` archive.
pub fn create_archive<P: AsRef<Path>>(
    target_path: P,
    items: &[StagedItem],
    passphrase: Option<&str>,
    zstd_level: u8,
    progress_callback: Option<ProgressCallback>,
) -> Result<ArchiveMetadata, FormatError> {
    let target_path = target_path.as_ref();
    let part_path = target_path.with_extension("klr.part");

    if let Some(parent) = part_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut out_file = File::create(&part_path)?;

    let encrypted = passphrase.map(|p| !p.is_empty()).unwrap_or(false);
    let (salt, base_nonce) = if encrypted {
        generate_crypto_params()
    } else {
        ([0u8; 16], [0u8; 7])
    };

    let key = if encrypted {
        Some(derive_key(passphrase.unwrap(), &salt)?)
    } else {
        None
    };

    let header = Header::new(encrypted, zstd_level, salt, base_nonce);
    header.write_to(&mut out_file)?;

    // Calculate total uncompressed size for progress tracking
    let mut total_uncompressed: u64 = 0;
    for item in items {
        if let Ok(meta) = fs::metadata(&item.disk_path) {
            if meta.is_file() {
                total_uncompressed += meta.len();
            }
        }
    }

    let mut index_entries = Vec::new();
    let mut frame_buffer = FrameBuffer::new(FRAME_PLAINTEXT_SIZE);
    let mut frame_counter: u32 = 0;

    let mut bytes_processed: u64 = 0;
    let mut last_progress_time = Instant::now();
    let mut last_bytes_processed: u64 = 0;
    let progress_cb_ref = progress_callback.clone();

    // Build tar stream into zstd encoder
    // We use a custom writer that accumulates compressed bytes and flushes frames
    struct FrameWriter<'a> {
        file: &'a mut File,
        frame_buffer: &'a mut FrameBuffer,
        frame_counter: &'a mut u32,
        key: Option<[u8; KEY_SIZE]>,
        base_nonce: [u8; 7],
    }

    impl<'a> Write for FrameWriter<'a> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.frame_buffer.push(buf);
            while let Some(frame) = self.frame_buffer.pop_frame() {
                if let Some(ref k) = self.key {
                    let enc = encrypt_frame(k, &self.base_nonce, *self.frame_counter, &frame)
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    let len_bytes = (enc.len() as u32).to_le_bytes();
                    self.file.write_all(&len_bytes)?;
                    self.file.write_all(&enc)?;
                } else {
                    self.file.write_all(&frame)?;
                }
                *self.frame_counter += 1;
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    let mut current_tar_offset: u64 = 0;

    {
        let frame_writer = FrameWriter {
            file: &mut out_file,
            frame_buffer: &mut frame_buffer,
            frame_counter: &mut frame_counter,
            key,
            base_nonce,
        };

        let mut zstd_encoder = zstd::stream::write::Encoder::new(frame_writer, zstd_level as i32)
            .map_err(|e| FormatError::Compression(e.to_string()))?;

        let counting_writer = CountingWriter {
            inner: &mut zstd_encoder,
            on_write: |n| {
                bytes_processed += n as u64;
                let elapsed = last_progress_time.elapsed().as_secs_f64();
                if elapsed >= 0.05 {
                    let delta_bytes = bytes_processed.saturating_sub(last_bytes_processed);
                    let speed = if elapsed > 0.0 { delta_bytes as f64 / elapsed } else { 0.0 };
                    if let Some(ref cb) = progress_cb_ref {
                        cb(ArchiveProgress {
                            bytes_processed,
                            total_bytes: total_uncompressed,
                            current_speed_bps: speed,
                            phase: ProgressPhase::Archiving,
                        });
                    }
                    last_progress_time = Instant::now();
                    last_bytes_processed = bytes_processed;
                }
            },
        };

        // Tar builder wrapping counting writer over zstd encoder
        let mut tar_builder = tar::Builder::new(counting_writer);

        for item in items {
            let metadata = fs::symlink_metadata(&item.disk_path)?;
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.len() };
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = if is_dir { 0o755 } else { 0o644 };

            index_entries.push(IndexEntry {
                path: item.archive_path.clone(),
                uncompressed_size: size,
                tar_offset: current_tar_offset,
                is_dir,
                mode,
                mtime,
            });

            if is_dir {
                tar_builder.append_dir(&item.archive_path, &item.disk_path)?;
                current_tar_offset += 512; // Standard tar dir header
            } else if metadata.file_type().is_symlink() {
                // Symlink support
                let target = fs::read_link(&item.disk_path)?;
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                tar_builder.append_link(&mut header, &item.archive_path, &target)?;
                current_tar_offset += 512;
            } else {
                let mut f = File::open(&item.disk_path)?;
                tar_builder.append_file(&item.archive_path, &mut f)?;
                // Tar files align to 512-byte boundaries
                let blocks = (size + 511) / 512;
                current_tar_offset += 512 + blocks * 512;
            }
        }

        tar_builder.finish()?;
        drop(tar_builder);
        let frame_writer = zstd_encoder.finish().map_err(|e| FormatError::Compression(e.to_string()))?;

        // Drain any remaining compressed frame in buffer as the final stream frame
        if let Some(remaining_frame) = frame_writer.frame_buffer.drain_remaining() {
            if let Some(ref k) = frame_writer.key {
                let enc = encrypt_frame(k, &frame_writer.base_nonce, *frame_writer.frame_counter, &remaining_frame)?;
                let len_bytes = (enc.len() as u32).to_le_bytes();
                frame_writer.file.write_all(&len_bytes)?;
                frame_writer.file.write_all(&enc)?;
            } else {
                frame_writer.file.write_all(&remaining_frame)?;
            }
            *frame_writer.frame_counter += 1;
        }
    }

    // Now write directory index footer
    let footer_offset = out_file.stream_position()?;
    let serialized_footer = bincode::serialize(&index_entries)?;
    let compressed_footer = compress_zstd(&serialized_footer, zstd_level)?;

    let final_footer_bytes = if let Some(ref k) = key {
        encrypt_footer(k, &base_nonce, &compressed_footer)?
    } else {
        compressed_footer
    };

    let footer_compressed_size = final_footer_bytes.len() as u64;
    out_file.write_all(&final_footer_bytes)?;

    // Write Trailer (28 bytes)
    let trailer = Trailer::new(footer_offset, footer_compressed_size, total_uncompressed);
    trailer.write_to(&mut out_file)?;
    out_file.flush()?;
    drop(out_file);

    // Atomically rename .target.klr.part to target.klr
    fs::rename(&part_path, target_path)?;

    if let Some(ref cb) = progress_callback {
        cb(ArchiveProgress {
            bytes_processed: total_uncompressed,
            total_bytes: total_uncompressed,
            current_speed_bps: 0.0,
            phase: ProgressPhase::Completed,
        });
    }

    let mut file_count = 0;
    let mut dir_count = 0;
    for entry in &index_entries {
        if entry.is_dir {
            dir_count += 1;
        } else {
            file_count += 1;
        }
    }

    let file_len = fs::metadata(target_path)?.len();

    Ok(ArchiveMetadata {
        header,
        trailer,
        entries: index_entries,
        total_uncompressed_bytes: total_uncompressed,
        total_compressed_bytes: file_len,
        file_count,
        dir_count,
    })
}

/// Extracts an entire `.klr` archive to the destination directory.
pub fn extract_archive<P: AsRef<Path>, D: AsRef<Path>>(
    archive_path: P,
    dest_dir: D,
    passphrase: Option<&str>,
    progress_callback: Option<ProgressCallback>,
) -> Result<(), FormatError> {
    extract_filtered(archive_path, dest_dir, passphrase, progress_callback, None)
}

/// Extracts a single file or directory subtree (`entry_path`) from the archive into `dest_dir`.
pub fn extract_entry<P: AsRef<Path>, D: AsRef<Path>>(
    archive_path: P,
    dest_dir: D,
    passphrase: Option<&str>,
    entry_path: &str,
) -> Result<(), FormatError> {
    extract_filtered(archive_path, dest_dir, passphrase, None, Some(entry_path.trim_end_matches('/')))
}

fn extract_filtered<P: AsRef<Path>, D: AsRef<Path>>(
    archive_path: P,
    dest_dir: D,
    passphrase: Option<&str>,
    progress_callback: Option<ProgressCallback>,
    filter: Option<&str>,
) -> Result<(), FormatError> {
    let archive_path = archive_path.as_ref();
    let dest_dir = dest_dir.as_ref();
    fs::create_dir_all(dest_dir)?;

    let mut file = File::open(archive_path)?;
    let header = Header::read_from(&mut file)?;
    let trailer = Trailer::read_from_end(&mut file)?;

    let key = if header.encrypted {
        let pass = passphrase.unwrap_or("");
        if pass.is_empty() {
            return Err(FormatError::Crypto("Passphrase required for extraction".to_string()));
        }
        Some(derive_key(pass, &header.salt)?)
    } else {
        None
    };

    let payload_start = HEADER_SIZE as u64;
    let payload_end = trailer.footer_offset;
    file.seek(SeekFrom::Start(payload_start))?;

    let total_bytes = trailer.total_uncompressed_bytes;
    let mut bytes_decompressed: u64 = 0;
    let mut last_progress_time = Instant::now();
    let mut last_bytes_decompressed: u64 = 0;

    // We pipe the payload stream into zstd decompressor and then into tar::Archive
    struct FramePayloadReader<'a> {
        file: &'a mut File,
        current_pos: u64,
        end_pos: u64,
        key: Option<[u8; KEY_SIZE]>,
        base_nonce: [u8; 7],
        frame_counter: u32,
        current_frame: Vec<u8>,
        current_frame_cursor: usize,
    }

    impl<'a> Read for FramePayloadReader<'a> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            while self.current_frame_cursor >= self.current_frame.len() {
                if self.current_pos >= self.end_pos {
                    return Ok(0); // EOF of payload stream
                }

                let plaintext = if let Some(ref k) = self.key {
                    let mut len_buf = [0u8; 4];
                    self.file.read_exact(&mut len_buf)?;
                    let frame_len = u32::from_le_bytes(len_buf) as usize;
                    let mut raw_frame = vec![0u8; frame_len];
                    self.file.read_exact(&mut raw_frame)?;
                    self.current_pos += 4 + frame_len as u64;

                    decrypt_frame(k, &self.base_nonce, self.frame_counter, &raw_frame)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
                } else {
                    let remaining_file_bytes = self.end_pos - self.current_pos;
                    let chunk_size = if remaining_file_bytes >= FRAME_PLAINTEXT_SIZE as u64 {
                        FRAME_PLAINTEXT_SIZE
                    } else {
                        remaining_file_bytes as usize
                    };
                    let mut raw_frame = vec![0u8; chunk_size];
                    self.file.read_exact(&mut raw_frame)?;
                    self.current_pos += chunk_size as u64;
                    raw_frame
                };

                self.frame_counter += 1;
                self.current_frame = plaintext;
                self.current_frame_cursor = 0;
            }

            let available = self.current_frame.len() - self.current_frame_cursor;
            let to_copy = buf.len().min(available);
            buf[..to_copy].copy_from_slice(
                &self.current_frame[self.current_frame_cursor..self.current_frame_cursor + to_copy],
            );
            self.current_frame_cursor += to_copy;
            Ok(to_copy)
        }
    }

    let payload_reader = FramePayloadReader {
        file: &mut file,
        current_pos: payload_start,
        end_pos: payload_end,
        key,
        base_nonce: header.base_nonce,
        frame_counter: 0,
        current_frame: Vec::new(),
        current_frame_cursor: 0,
    };

    let zstd_decoder = zstd::stream::read::Decoder::new(payload_reader)
        .map_err(|e| FormatError::Compression(e.to_string()))?;

    let progress_cb_ref = progress_callback.clone();
    let counting_reader = ProgressReader {
        inner: zstd_decoder,
        on_read: |n| {
            bytes_decompressed += n as u64;
            let elapsed = last_progress_time.elapsed().as_secs_f64();
            if elapsed >= 0.05 {
                let delta = bytes_decompressed.saturating_sub(last_bytes_decompressed);
                let speed = if elapsed > 0.0 { delta as f64 / elapsed } else { 0.0 };
                if let Some(ref cb) = progress_cb_ref {
                    cb(ArchiveProgress {
                        bytes_processed: bytes_decompressed,
                        total_bytes,
                        current_speed_bps: speed,
                        phase: ProgressPhase::Extracting,
                    });
                }
                last_progress_time = Instant::now();
                last_bytes_decompressed = bytes_decompressed;
            }
        },
    };

    // Tar unpack
    let mut tar_archive = tar::Archive::new(counting_reader);
    tar_archive.set_ignore_zeros(true);
    let mut entries = tar_archive.entries()?;

    while let Some(entry_res) = entries.next() {
        let mut entry = entry_res?;
        if let Some(f) = filter {
            let p = entry.path()?.to_string_lossy().trim_end_matches('/').to_string();
            if p != f && !p.starts_with(&format!("{}/", f)) {
                continue;
            }
        }
        entry.unpack_in(dest_dir)?;
    }

    if let Some(ref cb) = progress_callback {
        cb(ArchiveProgress {
            bytes_processed: total_bytes,
            total_bytes,
            current_speed_bps: 0.0,
            phase: ProgressPhase::Completed,
        });
    }

    Ok(())
}

/// Fast Append Path (Additions Only).
/// Seeks to `Footer Byte Offset` (truncating old footer), starts a new zstd + AES-128-GCM stream
/// for newly added tar entries, and writes an updated merged Directory Index Footer and Trailer.
pub fn append_archive<P: AsRef<Path>>(
    archive_path: P,
    new_items: &[StagedItem],
    passphrase: Option<&str>,
    progress_callback: Option<ProgressCallback>,
) -> Result<ArchiveMetadata, FormatError> {
    let archive_path = archive_path.as_ref();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(archive_path)?;

    let header = Header::read_from(&mut file)?;
    let trailer = Trailer::read_from_end(&mut file)?;

    // Inspect existing footer
    file.seek(SeekFrom::Start(trailer.footer_offset))?;
    let mut footer_bytes = vec![0u8; trailer.footer_compressed_size as usize];
    file.read_exact(&mut footer_bytes)?;

    let key = if header.encrypted {
        let pass = passphrase.unwrap_or("");
        if pass.is_empty() {
            return Err(FormatError::Crypto("Passphrase required for archive modification".to_string()));
        }
        Some(derive_key(pass, &header.salt)?)
    } else {
        None
    };

    let decompressed_footer = if let Some(ref k) = key {
        let decrypted = decrypt_footer(k, &header.base_nonce, &footer_bytes)?;
        decompress_zstd(&decrypted)?
    } else {
        decompress_zstd(&footer_bytes)?
    };

    let mut index_entries: Vec<IndexEntry> = bincode::deserialize(&decompressed_footer)?;

    // Truncate file at footer_offset to remove old footer and trailer
    file.set_len(trailer.footer_offset)?;
    file.seek(SeekFrom::Start(trailer.footer_offset))?;

    // Determine current frame counter by scanning length prefixes
    let mut frame_counter = 0u32;
    if header.encrypted {
        let mut scan_pos = HEADER_SIZE as u64;
        while scan_pos < trailer.footer_offset {
            file.seek(SeekFrom::Start(scan_pos))?;
            let mut len_buf = [0u8; 4];
            file.read_exact(&mut len_buf)?;
            let flen = u32::from_le_bytes(len_buf) as u64;
            scan_pos += 4 + flen;
            frame_counter += 1;
        }
        file.seek(SeekFrom::Start(trailer.footer_offset))?;
    }

    // Calculate uncompressed delta
    let mut added_uncompressed: u64 = 0;
    for item in new_items {
        if let Ok(meta) = fs::metadata(&item.disk_path) {
            if meta.is_file() {
                added_uncompressed += meta.len();
            }
        }
    }
    let new_total_uncompressed = trailer.total_uncompressed_bytes + added_uncompressed;

    let mut current_tar_offset = index_entries
        .iter()
        .map(|e| e.tar_offset + 512 + ((e.uncompressed_size + 511) / 512) * 512)
        .max()
        .unwrap_or(0);

    let mut frame_buffer = FrameBuffer::new(FRAME_PLAINTEXT_SIZE);

    // FrameWriter helper
    struct FrameWriter<'a> {
        file: &'a mut File,
        frame_buffer: &'a mut FrameBuffer,
        frame_counter: &'a mut u32,
        key: Option<[u8; KEY_SIZE]>,
        base_nonce: [u8; 7],
    }

    impl<'a> Write for FrameWriter<'a> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.frame_buffer.push(buf);
            while let Some(frame) = self.frame_buffer.pop_frame() {
                if let Some(ref k) = self.key {
                    let enc = encrypt_frame(k, &self.base_nonce, *self.frame_counter, &frame)
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                    let len_bytes = (enc.len() as u32).to_le_bytes();
                    self.file.write_all(&len_bytes)?;
                    self.file.write_all(&enc)?;
                } else {
                    self.file.write_all(&frame)?;
                }
                *self.frame_counter += 1;
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    {
        let frame_writer = FrameWriter {
            file: &mut file,
            frame_buffer: &mut frame_buffer,
            frame_counter: &mut frame_counter,
            key,
            base_nonce: header.base_nonce,
        };

        let mut zstd_encoder = zstd::stream::write::Encoder::new(frame_writer, header.zstd_level as i32)
            .map_err(|e| FormatError::Compression(e.to_string()))?;

        let mut bytes_processed: u64 = 0;
        let mut last_progress_time = Instant::now();
        let mut last_bytes_processed: u64 = 0;
        let progress_cb_ref = progress_callback.clone();

        let counting_writer = CountingWriter {
            inner: &mut zstd_encoder,
            on_write: |n| {
                bytes_processed += n as u64;
                let elapsed = last_progress_time.elapsed().as_secs_f64();
                if elapsed >= 0.05 {
                    let delta_bytes = bytes_processed.saturating_sub(last_bytes_processed);
                    let speed = if elapsed > 0.0 { delta_bytes as f64 / elapsed } else { 0.0 };
                    if let Some(ref cb) = progress_cb_ref {
                        cb(ArchiveProgress {
                            bytes_processed,
                            total_bytes: added_uncompressed,
                            current_speed_bps: speed,
                            phase: ProgressPhase::Archiving,
                        });
                    }
                    last_progress_time = Instant::now();
                    last_bytes_processed = bytes_processed;
                }
            },
        };

        let mut tar_builder = tar::Builder::new(counting_writer);

        for item in new_items {
            let metadata = fs::symlink_metadata(&item.disk_path)?;
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.len() };
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = if is_dir { 0o755 } else { 0o644 };

            index_entries.push(IndexEntry {
                path: item.archive_path.clone(),
                uncompressed_size: size,
                tar_offset: current_tar_offset,
                is_dir,
                mode,
                mtime,
            });

            if is_dir {
                tar_builder.append_dir(&item.archive_path, &item.disk_path)?;
                current_tar_offset += 512;
            } else if metadata.file_type().is_symlink() {
                let target = fs::read_link(&item.disk_path)?;
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                tar_builder.append_link(&mut header, &item.archive_path, &target)?;
                current_tar_offset += 512;
            } else {
                let mut f = File::open(&item.disk_path)?;
                tar_builder.append_file(&item.archive_path, &mut f)?;
                let blocks = (size + 511) / 512;
                current_tar_offset += 512 + blocks * 512;
            }
        }

        tar_builder.finish()?;
        drop(tar_builder);
        let frame_writer = zstd_encoder.finish().map_err(|e| FormatError::Compression(e.to_string()))?;

        if let Some(remaining_frame) = frame_writer.frame_buffer.drain_remaining() {
            if let Some(ref k) = frame_writer.key {
                let enc = encrypt_frame(k, &frame_writer.base_nonce, *frame_writer.frame_counter, &remaining_frame)?;
                let len_bytes = (enc.len() as u32).to_le_bytes();
                frame_writer.file.write_all(&len_bytes)?;
                frame_writer.file.write_all(&enc)?;
            } else {
                frame_writer.file.write_all(&remaining_frame)?;
            }
            *frame_writer.frame_counter += 1;
        }
    }

    // Write updated merged footer
    let new_footer_offset = file.stream_position()?;
    let serialized_footer = bincode::serialize(&index_entries)?;
    let compressed_footer = compress_zstd(&serialized_footer, header.zstd_level)?;

    let final_footer_bytes = if let Some(ref k) = key {
        encrypt_footer(k, &header.base_nonce, &compressed_footer)?
    } else {
        compressed_footer
    };

    let footer_compressed_size = final_footer_bytes.len() as u64;
    file.write_all(&final_footer_bytes)?;

    // Write updated Trailer
    let new_trailer = Trailer::new(new_footer_offset, footer_compressed_size, new_total_uncompressed);
    new_trailer.write_to(&mut file)?;
    file.flush()?;

    if let Some(ref cb) = progress_callback {
        cb(ArchiveProgress {
            bytes_processed: new_total_uncompressed,
            total_bytes: new_total_uncompressed,
            current_speed_bps: 0.0,
            phase: ProgressPhase::Completed,
        });
    }

    let mut file_count = 0;
    let mut dir_count = 0;
    for entry in &index_entries {
        if entry.is_dir {
            dir_count += 1;
        } else {
            file_count += 1;
        }
    }

    let total_len = file.metadata()?.len();

    Ok(ArchiveMetadata {
        header,
        trailer: new_trailer,
        entries: index_entries,
        total_uncompressed_bytes: new_total_uncompressed,
        total_compressed_bytes: total_len,
        file_count,
        dir_count,
    })
}

/// Full Repack Path (Deletions or Passphrase/Path Changed).
/// Unpacks existing archive into a temporary staging workspace, applies modifications, and repacks atomically.
pub fn repack_archive<P: AsRef<Path>, Q: AsRef<Path>>(
    src_archive_path: P,
    target_path: Q,
    kept_archive_paths: &[String],
    new_items: &[StagedItem],
    new_passphrase: Option<&str>,
    old_passphrase: Option<&str>,
    zstd_level: u8,
    progress_callback: Option<ProgressCallback>,
) -> Result<ArchiveMetadata, FormatError> {
    let temp_dir = std::env::temp_dir().join(format!(
        "kallara_repack_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir)?;

    let repack_work = || -> Result<ArchiveMetadata, FormatError> {
        // Extract source archive to temp_dir
        extract_archive(&src_archive_path, &temp_dir, old_passphrase, None)?;

        // Build list of all staged items for new archive
        let mut combined_items = Vec::new();

        // Include kept items from source archive
        for kept in kept_archive_paths {
            let disk_path = temp_dir.join(kept);
            if disk_path.exists() {
                combined_items.push(StagedItem {
                    disk_path,
                    archive_path: kept.clone(),
                });
            }
        }

        // Add new items
        combined_items.extend_from_slice(new_items);

        // Create the new archive at target_path
        create_archive(target_path, &combined_items, new_passphrase, zstd_level, progress_callback)
    };

    let result = repack_work();

    // Clean up temporary extraction
    let _ = fs::remove_dir_all(&temp_dir);

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_full_archive_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("kallara_test_{}", std::process::id()));
        let src_dir = temp_dir.join("src");
        let dest_dir = temp_dir.join("dest");
        let archive_path = temp_dir.join("test.klr");

        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(src_dir.join("sub")).unwrap();

        File::create(src_dir.join("hello.txt"))
            .unwrap()
            .write_all(b"Hello Kallara!")
            .unwrap();
        File::create(src_dir.join("sub").join("nested.bin"))
            .unwrap()
            .write_all(&vec![0xAA; 100_000])
            .unwrap();

        let staged = collect_staged_items(&src_dir).unwrap();
        assert_eq!(staged.len(), 3); // sub dir, hello.txt, nested.bin

        // 1. Create encrypted archive
        let passphrase = "my-secret-password";
        let meta = create_archive(&archive_path, &staged, Some(passphrase), 3, None).unwrap();
        assert_eq!(meta.file_count, 2);
        assert_eq!(meta.dir_count, 1);
        assert_eq!(meta.total_uncompressed_bytes, 14 + 100_000);

        // 2. O(1) Inspect
        let inspected = inspect_archive(&archive_path, Some(passphrase)).unwrap();
        assert_eq!(inspected.entries.len(), 3);
        assert_eq!(inspected.total_uncompressed_bytes, 14 + 100_000);

        // Wrong passphrase should fail inspection
        assert!(inspect_archive(&archive_path, Some("wrong-pw")).is_err());

        // 3. Extract and verify contents
        extract_archive(&archive_path, &dest_dir, Some(passphrase), None).unwrap();
        let extracted_hello = fs::read_to_string(dest_dir.join("hello.txt")).unwrap();
        assert_eq!(extracted_hello, "Hello Kallara!");
        let extracted_nested = fs::read(dest_dir.join("sub").join("nested.bin")).unwrap();
        assert_eq!(extracted_nested.len(), 100_000);
        assert_eq!(extracted_nested[0], 0xAA);

        // 4. Fast Append
        let extra_file = temp_dir.join("extra.txt");
        File::create(&extra_file)
            .unwrap()
            .write_all(b"Appended Content")
            .unwrap();

        let append_items = vec![StagedItem {
            disk_path: extra_file,
            archive_path: "extra.txt".to_string(),
        }];

        let meta_appended = append_archive(&archive_path, &append_items, Some(passphrase), None).unwrap();
        assert_eq!(meta_appended.entries.len(), 4);
        assert_eq!(meta_appended.file_count, 3);

        // Extract again to verify appended content
        let dest2 = temp_dir.join("dest2");
        extract_archive(&archive_path, &dest2, Some(passphrase), None).unwrap();
        let extracted_extra = fs::read_to_string(dest2.join("extra.txt")).unwrap();
        assert_eq!(extracted_extra, "Appended Content");

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_unencrypted_archive_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("kallara_unenc_{}", std::process::id()));
        let src_dir = temp_dir.join("src");
        let dest_dir = temp_dir.join("dest");
        let archive_path = temp_dir.join("unenc.klr");

        fs::create_dir_all(&src_dir).unwrap();
        File::create(src_dir.join("doc.txt"))
            .unwrap()
            .write_all(b"Raw tar.zst payload test")
            .unwrap();

        let staged = collect_staged_items(&src_dir).unwrap();
        let meta = create_archive(&archive_path, &staged, None, 3, None).unwrap();
        assert!(!meta.header.encrypted);
        assert_eq!(meta.file_count, 1);

        let inspected = inspect_archive(&archive_path, None).unwrap();
        assert!(!inspected.header.encrypted);
        assert_eq!(inspected.entries.len(), 1);

        extract_archive(&archive_path, &dest_dir, None, None).unwrap();
        let extracted = fs::read_to_string(dest_dir.join("doc.txt")).unwrap();
        assert_eq!(extracted, "Raw tar.zst payload test");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_repack_archive() {
        let temp_dir = std::env::temp_dir().join(format!("kallara_repack_t_{}", std::process::id()));
        let src_dir = temp_dir.join("src");
        let archive_v1 = temp_dir.join("v1.klr");
        let archive_v2 = temp_dir.join("v2.klr");
        let dest_dir = temp_dir.join("dest");

        fs::create_dir_all(&src_dir).unwrap();
        File::create(src_dir.join("keep.txt")).unwrap().write_all(b"keep me").unwrap();
        File::create(src_dir.join("delete.txt")).unwrap().write_all(b"delete me").unwrap();

        let staged = collect_staged_items(&src_dir).unwrap();
        create_archive(&archive_v1, &staged, Some("pass1"), 3, None).unwrap();

        let new_file = temp_dir.join("added.txt");
        File::create(&new_file).unwrap().write_all(b"new addition").unwrap();
        let new_staged = vec![StagedItem {
            disk_path: new_file,
            archive_path: "added.txt".to_string(),
        }];

        repack_archive(
            &archive_v1,
            &archive_v2,
            &["keep.txt".to_string()],
            &new_staged,
            Some("pass2"),
            Some("pass1"),
            3,
            None,
        ).unwrap();

        extract_archive(&archive_v2, &dest_dir, Some("pass2"), None).unwrap();
        assert!(dest_dir.join("keep.txt").exists());
        assert!(dest_dir.join("added.txt").exists());
        assert!(!dest_dir.join("delete.txt").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extract_entry_selective() {
        let temp_dir = std::env::temp_dir().join(format!("kallara_sel_t_{}", std::process::id()));
        let src_dir = temp_dir.join("src");
        let archive_path = temp_dir.join("sel.klr");
        let dest_dir = temp_dir.join("dest");

        fs::create_dir_all(&src_dir).unwrap();
        File::create(src_dir.join("file1.txt")).unwrap().write_all(b"content 1").unwrap();
        File::create(src_dir.join("file2.txt")).unwrap().write_all(b"content 2").unwrap();

        let staged = collect_staged_items(&src_dir).unwrap();
        create_archive(&archive_path, &staged, Some("secret"), 3, None).unwrap();

        // Extract only file1.txt
        extract_entry(&archive_path, &dest_dir, Some("secret"), "file1.txt").unwrap();
        assert!(dest_dir.join("file1.txt").exists());
        assert!(!dest_dir.join("file2.txt").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_repack_archive_toggle_encryption_and_level() {
        let temp_dir = std::env::temp_dir().join(format!("kallara_rep_toggle_{}", std::process::id()));
        let src_dir = temp_dir.join("src");
        let unenc_archive = temp_dir.join("unenc.klr");
        let enc_archive = temp_dir.join("enc.klr");
        let unenc2_archive = temp_dir.join("unenc2.klr");
        let dest_dir = temp_dir.join("dest");

        fs::create_dir_all(&src_dir).unwrap();
        File::create(src_dir.join("data.txt"))
            .unwrap()
            .write_all(b"Important archive data for repack testing")
            .unwrap();

        let staged = collect_staged_items(&src_dir).unwrap();
        // 1. Create initial unencrypted archive at level 1
        let meta1 = create_archive(&unenc_archive, &staged, None, 1, None).unwrap();
        assert!(!meta1.header.encrypted);
        assert_eq!(meta1.header.zstd_level, 1);

        // 2. Repack from unencrypted to encrypted at level 7
        let meta2 = repack_archive(
            &unenc_archive,
            &enc_archive,
            &["data.txt".to_string()],
            &[],
            Some("brand-new-key"),
            None,
            7,
            None,
        ).unwrap();
        assert!(meta2.header.encrypted);
        assert_eq!(meta2.header.zstd_level, 7);

        // Verify encrypted archive cannot be inspected without password
        assert!(inspect_archive(&enc_archive, None).is_err());
        let insp = inspect_archive(&enc_archive, Some("brand-new-key")).unwrap();
        assert!(insp.header.encrypted);
        assert_eq!(insp.header.zstd_level, 7);

        // 3. Repack back from encrypted to unencrypted at level 3
        let meta3 = repack_archive(
            &enc_archive,
            &unenc2_archive,
            &["data.txt".to_string()],
            &[],
            None,
            Some("brand-new-key"),
            3,
            None,
        ).unwrap();
        assert!(!meta3.header.encrypted);
        assert_eq!(meta3.header.zstd_level, 3);

        extract_archive(&unenc2_archive, &dest_dir, None, None).unwrap();
        let extracted = fs::read_to_string(dest_dir.join("data.txt")).unwrap();
        assert_eq!(extracted, "Important archive data for repack testing");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
