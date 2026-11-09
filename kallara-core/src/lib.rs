pub mod archive;
pub mod compress;
pub mod crypto;
pub mod format;
pub mod tree;

pub use archive::{
    append_archive, collect_staged_items, create_archive, extract_archive, extract_entry, inspect_archive,
    repack_archive, ArchiveMetadata, ArchiveProgress, ProgressCallback, ProgressPhase, StagedItem,
};
pub use compress::{compress_zstd, decompress_zstd, estimate_compressed_size, DEFAULT_ZSTD_LEVEL};
pub use crypto::{
    decrypt_footer, decrypt_frame, derive_footer_nonce, derive_key, encrypt_footer, encrypt_frame,
    generate_crypto_params, KEY_SIZE,
};
pub use format::{
    FormatError, Header, IndexEntry, Trailer, ENCRYPTED_FRAME_SIZE, FRAME_PLAINTEXT_SIZE,
    GCM_TAG_SIZE, HEADER_SIZE, TRAILER_SIZE,
};
pub use tree::{FlatTreeRow, NodeStatus, NodeType, WorkspaceTree};
