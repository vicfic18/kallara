#![allow(non_snake_case)]

mod assets;
mod styles;

use dioxus::desktop::tao::window::ResizeDirection;
use dioxus::desktop::{Config, WindowBuilder};
use dioxus::html::HasFileData;
use dioxus::prelude::*;
use kallara_core::{
    append_archive, collect_staged_items, create_archive, estimate_compressed_size, extract_archive,
    extract_entry, inspect_archive, repack_archive, ArchiveMetadata, ArchiveProgress, FlatTreeRow,
    NodeStatus, StagedItem, WorkspaceTree, DEFAULT_ZSTD_LEVEL,
};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

static DROP_TX: Mutex<Option<UnboundedSender<Vec<PathBuf>>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Staging,       // State 1: New Archive
    InspectClean,  // State 2: Inspect existing archive without modifications
    InspectDirty,  // State 3: Existing archive modified with staged additions/deletions
}

#[derive(Debug, Clone)]
enum WorkerEvent {
    Progress(ArchiveProgress),
    ArchiveDone(Result<ArchiveMetadata, String>),
    ExtractDone(Result<PathBuf, String>),
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_bytes_delta(delta: i64) -> String {
    if delta == 0 {
        "".to_string()
    } else if delta > 0 {
        format!("(+{})", format_bytes(delta as u64))
    } else {
        format!("(-{})", format_bytes((-delta) as u64))
    }
}

fn row_class(staged: bool, selected: bool) -> &'static str {
    match (staged, selected) {
        (true, true) => "tree-row staged selected",
        (true, false) => "tree-row staged",
        (false, true) => "tree-row selected",
        (false, false) => "tree-row",
    }
}

fn clean_path(p: &std::path::Path) -> PathBuf {
    let s = p.to_string_lossy();
    let stripped = if let Some(stripped) = s.strip_prefix("file://localhost") {
        stripped
    } else if let Some(stripped) = s.strip_prefix("file://") {
        stripped
    } else {
        &s
    };
    if let Ok(decoded) = percent_encoding::percent_decode_str(stripped).decode_utf8() {
        let p_decoded = PathBuf::from(decoded.as_ref());
        if p_decoded.exists() {
            return p_decoded;
        }
    }
    let p_stripped = PathBuf::from(stripped);
    if p_stripped.exists() {
        return p_stripped;
    }
    if let Ok(decoded) = percent_encoding::percent_decode_str(stripped).decode_utf8() {
        return PathBuf::from(decoded.as_ref());
    }
    p_stripped
}

fn glyph_class(staged: bool, is_dir: bool) -> &'static str {
    if staged { "tree-glyph staged" } else if is_dir { "tree-glyph dir" } else { "tree-glyph" }
}

fn main() {
    let window = WindowBuilder::new()
        .with_title("Kallara")
        .with_decorations(false)
        .with_inner_size(dioxus::desktop::LogicalSize::new(980.0, 680.0))
        .with_min_inner_size(dioxus::desktop::LogicalSize::new(800.0, 520.0));

    let config = Config::new()
        .with_window(window)
        .with_menu(None)
        .with_disable_context_menu(true)
        .with_disable_drag_drop_handler(true)
        .with_custom_protocol("kallara-drop", |req: dioxus::desktop::wry::http::Request<Vec<u8>>| {
            println!("[DND-PROTOCOL] Received kallara-drop:// request ({} bytes)", req.body().len());
            if let Ok(paths) = serde_json::from_slice::<Vec<String>>(req.body()) {
                println!("[DND-PROTOCOL] Parsed dropped paths: {:?}", paths);
                let buf_paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                if let Ok(guard) = DROP_TX.lock() {
                    if let Some(ref tx) = *guard {
                        let _ = tx.send(buf_paths);
                    }
                }
            }
            dioxus::desktop::wry::http::Response::builder()
                .header("Content-Type", "text/plain")
                .header("Access-Control-Allow-Origin", "*")
                .header("Access-Control-Allow-Methods", "POST, OPTIONS")
                .header("Access-Control-Allow-Headers", "*")
                .body(Cow::Borrowed(&b"ok"[..]))
                .unwrap()
        })
        .with_custom_head(format!(
            "<style>{}</style><script>{}</script>",
            styles::generate_css(),
            r#"
            (function() {
                function logDnd(msg) {
                    console.log(msg);
                    if (window.__kallara_log) {
                        try { window.__kallara_log(msg); } catch(e) {}
                    }
                }

                // Workaround for WebKitGTK Bug 265857 / Dioxus #1504:
                // Ensure any internal draggable element populates dataTransfer on dragstart
                window.addEventListener('dragstart', function(ev) {
                    logDnd('[DND-JS] dragstart on target id=' + (ev.target ? ev.target.id : 'none'));
                    if (ev.dataTransfer) {
                        var id = (ev.target && ev.target.id) ? ev.target.id : 'kallara-drag';
                        ev.dataTransfer.setData('text/plain', id);
                    }
                }, true);

                function decodePath(uri) {
                    if (!uri) return '';
                    var s = uri.trim();
                    if (s.startsWith('file://localhost')) {
                        s = s.substring(16);
                    } else if (s.startsWith('file://')) {
                        s = s.substring(7);
                    }
                    try {
                        return decodeURI(s);
                    } catch(e) {
                        return s;
                    }
                }

                function extractDroppedPaths(e) {
                    var paths = [];
                    if (!e || !e.dataTransfer) {
                        logDnd('[DND-JS] extractDroppedPaths: no dataTransfer');
                        return paths;
                    }
                    var dt = e.dataTransfer;
                    var types = dt.types ? Array.from(dt.types) : [];
                    var filesLen = dt.files ? dt.files.length : 0;
                    var uriList = '';
                    var plain = '';
                    try { uriList = dt.getData('text/uri-list') || ''; } catch(err) {}
                    try { plain = dt.getData('text/plain') || ''; } catch(err) {}

                    logDnd('[DND-JS] drop dataTransfer types=' + JSON.stringify(types) +
                           ' files.length=' + filesLen +
                           ' uri-list=' + JSON.stringify(uriList) +
                           ' text/plain=' + JSON.stringify(plain));

                    if (uriList) {
                        var lines = uriList.split(/\r?\n/);
                        for (var i = 0; i < lines.length; i++) {
                            var line = lines[i].trim();
                            if (line && !line.startsWith('#')) {
                                var p = decodePath(line);
                                if (p) paths.push(p);
                            }
                        }
                    }
                    if (paths.length === 0 && dt.files && dt.files.length > 0) {
                        for (var i = 0; i < dt.files.length; i++) {
                            var f = dt.files[i];
                            if (f.path) paths.push(f.path);
                            else if (f.name) paths.push(f.name);
                        }
                    }
                    if (paths.length === 0 && plain) {
                        var plines = plain.split(/\r?\n/);
                        for (var i = 0; i < plines.length; i++) {
                            var l = plines[i].trim();
                            if (l.startsWith('/')) paths.push(l);
                            else if (l.startsWith('file://')) paths.push(decodePath(l));
                        }
                    }
                    logDnd('[DND-JS] extracted paths=' + JSON.stringify(paths));
                    return paths;
                }

                function postPaths(paths) {
                    if (!paths || paths.length === 0) return;
                    logDnd('[DND-JS] postPaths sending ' + paths.length + ' path(s)');
                    try {
                        fetch('kallara-drop://drop', {
                            method: 'POST',
                            body: JSON.stringify(paths)
                        }).catch(function(err) { console.error('fetch drop error:', err); });
                    } catch(err) {
                        console.error('postPaths error:', err);
                    }
                    if (window.__kallara_send_drop) {
                        try { window.__kallara_send_drop(paths); } catch(e) {}
                    }
                }

                function clearDragHighlight() {
                    document.body.classList.remove('drag-active');
                    var container = document.querySelector('.app-container');
                    if (container) container.classList.remove('drag-active');
                    var well = document.querySelector('.tree-well');
                    if (well) well.classList.remove('drag-over');
                    if (window.__kallara_signal_drag) {
                        try { window.__kallara_signal_drag(false); } catch(e) {}
                    }
                }

                window.addEventListener('dragenter', function(e) {
                    e.preventDefault();
                    if (e.dataTransfer) {
                        e.dataTransfer.dropEffect = 'copy';
                    }
                    var types = (e.dataTransfer && e.dataTransfer.types) ? Array.from(e.dataTransfer.types) : [];
                    logDnd('[DND-JS] window dragenter types=' + JSON.stringify(types));
                    document.body.classList.add('drag-active');
                    if (window.__kallara_signal_drag) {
                        try { window.__kallara_signal_drag(true); } catch(e) {}
                    }
                }, true);

                window.addEventListener('dragover', function(e) {
                    e.preventDefault();
                    if (e.dataTransfer) {
                        e.dataTransfer.dropEffect = 'copy';
                    }
                    document.body.classList.add('drag-active');
                }, true);

                window.addEventListener('dragleave', function(e) {
                    if (!e.relatedTarget || e.clientX <= 0 || e.clientY <= 0 || e.clientX >= window.innerWidth || e.clientY >= window.innerHeight) {
                        logDnd('[DND-JS] window dragleave (exited window)');
                        clearDragHighlight();
                    }
                }, true);

                window.addEventListener('drop', function(e) {
                    logDnd('[DND-JS] window drop event fired');
                    e.preventDefault();
                    e.stopPropagation();
                    clearDragHighlight();
                    var paths = extractDroppedPaths(e);
                    if (paths && paths.length > 0) {
                        postPaths(paths);
                    }
                }, true);

                window.addEventListener('mouseup', function(e) {
                    clearDragHighlight();
                }, true);
            })();
            "#
        ));

    dioxus::LaunchBuilder::desktop()
        .with_cfg(config)
        .launch(App);
}

fn App() -> Element {
    let desktop = dioxus::desktop::window();
    let mut is_maximized = use_signal(|| desktop.is_maximized());

    let handle_minimize = {
        let d = desktop.clone();
        move |e: MouseEvent| {
            e.stop_propagation();
            d.set_minimized(true);
        }
    };

    let handle_maximize = {
        let d = desktop.clone();
        move |e: MouseEvent| {
            e.stop_propagation();
            d.toggle_maximized();
            is_maximized.set(d.is_maximized());
        }
    };

    let handle_close = {
        let d = desktop.clone();
        move |e: MouseEvent| {
            e.stop_propagation();
            d.close();
        }
    };

    // Core Application State
    let mut app_state = use_signal(|| AppState::Staging);
    let mut tree = use_signal(WorkspaceTree::new);
    let mut archive_path: Signal<Option<PathBuf>> = use_signal(|| None);
    let mut opened_archive_path: Signal<Option<PathBuf>> = use_signal(|| None);
    let mut baseline_encrypted: Signal<Option<bool>> = use_signal(|| None);
    let mut baseline_passphrase: Signal<Option<String>> = use_signal(|| None);
    let mut baseline_zstd_level: Signal<Option<u8>> = use_signal(|| None);

    let mut is_encrypted = use_signal(|| false);
    let mut passphrase = use_signal(String::new);
    let mut zstd_level = use_signal(|| DEFAULT_ZSTD_LEVEL);
    let mut key_pulsing = use_signal(|| false);

    // Active Processing / Archival State
    let mut is_processing = use_signal(|| false);
    let mut is_extracting = use_signal(|| false);
    let mut show_graph = use_signal(|| false);
    let mut progress_pct = use_signal(|| 0.0f64);
    let mut current_speed_mb = use_signal(|| 0.0f64);
    let mut peak_speed_mb = use_signal(|| 0.0f64);
    let mut speed_samples = use_signal(|| VecDeque::from(vec![0.0f64; 60]));

    // Drag and drop state
    let mut is_dragging = use_signal(|| false);

    // Telemetry display values
    let mut committed_compressed_size = use_signal(|| 0u64);
    let estimated_compressed_size = use_signal(|| 0u64);

    // Exit / Overwrite Guard banner state
    let mut show_dirty_banner = use_signal(|| false);
    let mut status_message = use_signal(|| "Ready".to_string());

    // Virtualization scroll state for tree (scroll offset + measured viewport height)
    let mut scroll_top = use_signal(|| 0.0f64);
    let mut viewport_h = use_signal(|| 1200.0f64);
    let mut tree_mount: Signal<Option<std::rc::Rc<MountedData>>> = use_signal(|| None);
    let viewport_height = *viewport_h.read();
    let row_height = 26.0f64; // must match .tree-row height in styles.rs

    // Right-click context menu: (x, y, row)
    let mut ctx_menu: Signal<Option<(f64, f64, FlatTreeRow)>> = use_signal(|| None);

    // Async worker channel for live progress updates and thread completion
    let worker_tx = use_signal(|| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WorkerEvent>();
        spawn(async move {
            while let Some(evt) = rx.recv().await {
                match evt {
                    WorkerEvent::Progress(p) => {
                        let speed_mb = p.current_speed_bps / (1024.0 * 1024.0);
                        current_speed_mb.set(speed_mb);
                        if speed_mb > *peak_speed_mb.read() {
                            peak_speed_mb.set(speed_mb);
                        }
                        if p.total_bytes > 0 {
                            let pct = (p.bytes_processed as f64 / p.total_bytes as f64) * 100.0;
                            progress_pct.set(pct.clamp(0.0, 100.0));
                        }
                        let mut q = speed_samples.write();
                        q.pop_front();
                        q.push_back(speed_mb);
                    }
                    WorkerEvent::ArchiveDone(res) => {
                        is_processing.set(false);
                        match res {
                            Ok(meta) => {
                                progress_pct.set(100.0);
                                current_speed_mb.set(0.0);
                                tree.write().set_committed(meta.entries);
                                committed_compressed_size.set(meta.total_compressed_bytes);
                                app_state.set(AppState::InspectClean);
                                show_dirty_banner.set(false);
                                baseline_encrypted.set(Some(meta.header.encrypted));
                                baseline_passphrase.set(if meta.header.encrypted {
                                    Some(passphrase.read().clone())
                                } else {
                                    None
                                });
                                baseline_zstd_level.set(Some(meta.header.zstd_level));
                                if let Some(ref p) = *archive_path.read() {
                                    opened_archive_path.set(Some(p.clone()));
                                }
                                status_message.set("Archive completed successfully".to_string());
                            }
                            Err(e) => {
                                status_message.set(format!("Archiving failed: {}", e));
                            }
                        }
                    }
                    WorkerEvent::ExtractDone(res) => {
                        is_processing.set(false);
                        match res {
                            Ok(dest) => {
                                progress_pct.set(100.0);
                                current_speed_mb.set(0.0);
                                status_message.set(format!("Extracted to: {}", dest.display()));
                            }
                            Err(e) => {
                                status_message.set(format!("Extraction failed: {}", e));
                            }
                        }
                    }
                }
            }
        });
        tx
    });

    // Helper: update compression estimate
    let update_estimate = move || {
        let (uncompressed, delta) = tree.read().total_uncompressed_bytes();
        let total = (uncompressed as i64 + delta).max(0) as u64;
        let sample = b"sample text for estimation purposes";
        let est = estimate_compressed_size(total, sample, *zstd_level.read());
        let mut sig = estimated_compressed_size;
        sig.set(est);
    };

    // Calculate visible rows for DOM virtualization with generous overscan
    let flat_rows = tree.read().flatten_visible_rows();
    let total_rows = flat_rows.len();
    let (start_idx, end_idx) = if total_rows <= 120 {
        (0, total_rows)
    } else {
        let raw_start = (*scroll_top.read() / row_height).floor() as usize;
        let start = raw_start.saturating_sub(15).min(total_rows);
        let count = (viewport_height / row_height).ceil() as usize + 35;
        let end = (start + count).min(total_rows);
        (start, end)
    };
    let visible_rows = if total_rows > 0 && start_idx < total_rows {
        &flat_rows[start_idx..end_idx]
    } else {
        &[]
    };

    // Telemetry stats
    let (committed_uncompressed, uncompressed_delta) = tree.read().total_uncompressed_bytes();
    let total_uncompressed = (committed_uncompressed as i64 + uncompressed_delta).max(0) as u64;
    let (file_count, dir_count) = tree.read().count_nodes();

    // Compression Ratio
    let compressed_for_ratio = match *app_state.read() {
        AppState::Staging => *estimated_compressed_size.read(),
        AppState::InspectClean => *committed_compressed_size.read(),
        AppState::InspectDirty => *committed_compressed_size.read(),
    };
    let ratio_str = if compressed_for_ratio > 0 && total_uncompressed > 0 {
        let ratio = total_uncompressed as f64 / compressed_for_ratio as f64;
        let pct = (compressed_for_ratio as f64 / total_uncompressed as f64) * 100.0;
        format!("{:.2}x ({:.1}%)", ratio, pct)
    } else {
        "-".to_string()
    };

    let compressed_display_str = match *app_state.read() {
        AppState::Staging => format!("~{}", format_bytes(*estimated_compressed_size.read())),
        AppState::InspectClean => format_bytes(*committed_compressed_size.read()),
        AppState::InspectDirty => {
            let delta = (*estimated_compressed_size.read()).saturating_sub(*committed_compressed_size.read());
            format!("{} (~+{})", format_bytes(*committed_compressed_size.read()), format_bytes(delta))
        }
    };

    let file_display_name = archive_path.read().as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive.klr".to_string());

    let tree_header_title = match *app_state.read() {
        AppState::Staging => "[/] workspace (Staged Archive)".to_string(),
        AppState::InspectClean => format!("[/] {} (Committed)", file_display_name),
        AppState::InspectDirty => format!("[*] {} (Unarchived Changes)", file_display_name),
    };

    let action_grid_class = match *app_state.read() {
        AppState::Staging => "action-grid single-col",
        AppState::InspectClean | AppState::InspectDirty => "action-grid",
    };

    let archive_btn_class = match *app_state.read() {
        AppState::Staging => "raised-button primary",
        AppState::InspectClean => "raised-button primary",
        AppState::InspectDirty => "raised-button primary dirty-active",
    };

    let is_archive_btn_disabled = *app_state.read() == AppState::InspectClean || *is_processing.read();

    // Helper: detect dirty modifications on an opened archive
    let check_dirty = move || {
        if *app_state.read() != AppState::Staging {
            let t = tree.read();
            let has_staged = !t.staged_items.is_empty();
            let has_deleted = !t.deleted_paths.is_empty();
            let path_changed = archive_path.read().is_some() && *archive_path.read() != *opened_archive_path.read();
            let enc_changed = *baseline_encrypted.read() != Some(*is_encrypted.read());
            let pass_changed = if *is_encrypted.read() {
                baseline_passphrase.read().as_deref().unwrap_or("") != passphrase.read().as_str()
            } else {
                *baseline_encrypted.read() == Some(true)
            };
            let lvl_changed = *baseline_zstd_level.read() != Some(*zstd_level.read());

            if has_staged || has_deleted || path_changed || enc_changed || pass_changed || lvl_changed {
                app_state.set(AppState::InspectDirty);
            } else {
                app_state.set(AppState::InspectClean);
            }
        }
    };

    // Helper: open and inspect a .klr archive
    let open_klr_archive = move |file: PathBuf| {
        archive_path.set(Some(file.clone()));
        opened_archive_path.set(Some(file.clone()));
        show_graph.set(false);

        let mut header_opt = None;
        if let Ok(mut f) = std::fs::File::open(&file) {
            if let Ok(hdr) = kallara_core::Header::read_from(&mut f) {
                header_opt = Some(hdr);
                is_encrypted.set(hdr.encrypted);
                zstd_level.set(hdr.zstd_level);
            }
        }

        let pass = passphrase.read().clone();
        let pass_opt = if pass.is_empty() { None } else { Some(pass.as_str()) };

        match inspect_archive(&file, pass_opt) {
            Ok(meta) => {
                tree.write().set_committed(meta.entries);
                committed_compressed_size.set(meta.total_compressed_bytes);
                key_pulsing.set(false);
                is_encrypted.set(meta.header.encrypted);
                zstd_level.set(meta.header.zstd_level);
                baseline_encrypted.set(Some(meta.header.encrypted));
                baseline_passphrase.set(if meta.header.encrypted {
                    Some(pass.clone())
                } else {
                    None
                });
                baseline_zstd_level.set(Some(meta.header.zstd_level));
                if !meta.header.encrypted {
                    passphrase.set(String::new());
                }
                app_state.set(AppState::InspectClean);
                show_dirty_banner.set(false);
                status_message.set(format!("Archive opened: {} nodes", meta.file_count + meta.dir_count));
            }
            Err(kallara_core::FormatError::Crypto(_)) => {
                is_encrypted.set(true);
                baseline_encrypted.set(Some(true));
                if let Some(hdr) = header_opt {
                    zstd_level.set(hdr.zstd_level);
                    baseline_zstd_level.set(Some(hdr.zstd_level));
                }
                key_pulsing.set(true);
                status_message.set("Passphrase required: enter KEY and press Enter to unlock".to_string());
            }
            Err(e) => {
                status_message.set(format!("Failed to open archive: {}", e));
            }
        }
    };

    // Helper: stage dropped or picked paths
    let stage_paths = {
        let mut opener = open_klr_archive.clone();
        let mut checker = check_dirty.clone();
        move |paths: Vec<PathBuf>| {
            println!("[DND-STAGE] stage_paths called with {} raw path(s): {:?}", paths.len(), paths);
            let is_empty_workspace = tree.read().committed_entries.is_empty() && tree.read().staged_items.is_empty();
            let mut added_count = 0;

            for raw_path in paths {
                let path = clean_path(&raw_path);
                let exists = path.exists();
                println!("[DND-STAGE] cleaned path: {:?} (exists={})", path, exists);
                if !exists {
                    continue;
                }

                if path.extension().and_then(|s| s.to_str()) == Some("klr") && (is_empty_workspace || *app_state.read() == AppState::Staging) {
                    println!("[DND-STAGE] Opening .klr archive: {:?}", path);
                    opener(path);
                    return;
                }

                if path.is_dir() {
                    let base_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    if let Ok(items) = collect_staged_items(&path) {
                        for item in items {
                            let meta = std::fs::metadata(&item.disk_path);
                            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                            let size = meta.map(|m| m.len()).unwrap_or(0);
                            let arch_path = format!("{}/{}", base_name, item.archive_path);
                            tree.write().add_staged(item.disk_path, arch_path, size, is_dir);
                            added_count += 1;
                        }
                    }
                } else if path.is_file() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    tree.write().add_staged(path, name, size, false);
                    added_count += 1;
                }
            }

            println!("[DND-STAGE] Finished staging: added_count={}", added_count);
            if added_count > 0 {
                checker();
                update_estimate();
                status_message.set(format!("Staged {} item(s) to workspace", added_count));
            }
        }
    };

    // Action button handlers
    let handle_stage_files = {
        let mut stager = stage_paths.clone();
        move |_| {
            if let Some(files) = rfd::FileDialog::new().pick_files() {
                stager(files);
            }
        }
    };

    let handle_stage_folder = {
        let mut stager = stage_paths.clone();
        move |_| {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                stager(vec![folder]);
            }
        }
    };

    let handle_open_archive = {
        let mut opener = open_klr_archive.clone();
        move |_| {
            if let Some(file) = rfd::FileDialog::new().add_filter("Kallara Archive", &["klr"]).pick_file() {
                opener(file);
            }
        }
    };

    // Asynchronous Archival Handler
    let handle_archive_action = move |_| {
        if *is_processing.read() {
            return;
        }

        let current_state = *app_state.read();

        if current_state == AppState::Staging && tree.read().staged_items.is_empty() {
            status_message.set("Stage files or folders before creating an archive".to_string());
            return;
        }

        if *is_encrypted.read() && passphrase.read().is_empty() {
            key_pulsing.set(true);
            status_message.set("Passphrase required: enter key in KEY input".to_string());
            return;
        }

        let pass = if *is_encrypted.read() { passphrase.read().clone() } else { String::new() };
        let lvl = *zstd_level.read();

        let target = if current_state == AppState::Staging {
            let default_name = archive_path.read().as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "archive.klr".to_string());

            let mut dialog = rfd::FileDialog::new()
                .add_filter("Kallara Archive", &["klr"])
                .set_file_name(&default_name);

            if let Some(existing) = archive_path.read().as_ref() {
                if let Some(parent) = existing.parent() {
                    dialog = dialog.set_directory(parent);
                }
            }

            if let Some(picked) = dialog.save_file() {
                let final_path = if picked.extension().is_none() {
                    picked.with_extension("klr")
                } else {
                    picked
                };
                archive_path.set(Some(final_path.clone()));
                final_path
            } else {
                return;
            }
        } else {
            match archive_path.read().clone() {
                Some(p) => p,
                None => {
                    status_message.set("No archive target path selected".to_string());
                    return;
                }
            }
        };

        is_processing.set(true);
        is_extracting.set(false);
        show_graph.set(true);
        progress_pct.set(0.0);
        current_speed_mb.set(0.0);
        peak_speed_mb.set(0.0);
        speed_samples.write().clear();
        speed_samples.write().extend(vec![0.0f64; 60]);
        status_message.set("Archiving in progress...".to_string());

        let tx = worker_tx.read().clone();
        let tx_prog = tx.clone();
        let progress_cb = Arc::new(move |p: ArchiveProgress| {
            let _ = tx_prog.send(WorkerEvent::Progress(p));
        });

        match current_state {
            AppState::Staging => {
                let staged_items: Vec<StagedItem> = tree
                    .read()
                    .staged_items
                    .iter()
                    .map(|(d, a, _, _)| StagedItem {
                        disk_path: d.clone(),
                        archive_path: a.clone(),
                    })
                    .collect();

                std::thread::spawn(move || {
                    let pass_opt = if pass.is_empty() { None } else { Some(pass.as_str()) };
                    let res = create_archive(&target, &staged_items, pass_opt, lvl, Some(progress_cb))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(WorkerEvent::ArchiveDone(res));
                });
            }
            AppState::InspectDirty => {
                let src = opened_archive_path.read().clone().unwrap_or_else(|| target.clone());
                let old_pass = baseline_passphrase.read().clone();
                let enc_changed = *baseline_encrypted.read() != Some(*is_encrypted.read());
                let pass_changed = if *is_encrypted.read() {
                    baseline_passphrase.read().as_deref().unwrap_or("") != pass.as_str()
                } else {
                    *baseline_encrypted.read() == Some(true)
                };
                let lvl_changed = *baseline_zstd_level.read() != Some(lvl);
                let path_changed = src != target;
                let has_deletions = !tree.read().deleted_paths.is_empty();
                let newly_staged: Vec<StagedItem> = tree
                    .read()
                    .staged_items
                    .iter()
                    .map(|(d, a, _, _)| StagedItem {
                        disk_path: d.clone(),
                        archive_path: a.clone(),
                    })
                    .collect();

                let can_fast_append = !has_deletions
                    && !enc_changed
                    && !pass_changed
                    && !lvl_changed
                    && !path_changed
                    && !newly_staged.is_empty();

                if can_fast_append {
                    // Fast Append Path
                    std::thread::spawn(move || {
                        let pass_opt = if pass.is_empty() { None } else { Some(pass.as_str()) };
                        let res = append_archive(&target, &newly_staged, pass_opt, Some(progress_cb))
                            .map_err(|e| e.to_string());
                        let _ = tx.send(WorkerEvent::ArchiveDone(res));
                    });
                } else {
                    // Full Repack Path (Deletions, Key Change, LVL Change, or Target Path Change)
                    let kept_paths: Vec<String> = tree
                        .read()
                        .committed_entries
                        .iter()
                        .filter(|e| !tree.read().deleted_paths.contains(&e.path))
                        .map(|e| e.path.clone())
                        .collect();

                    std::thread::spawn(move || {
                        let new_pass_opt = if pass.is_empty() { None } else { Some(pass.as_str()) };
                        let old_pass_opt = old_pass.as_deref();
                        let part_target = target.with_extension("repack.klr");
                        let res = repack_archive(
                            &src,
                            &part_target,
                            &kept_paths,
                            &newly_staged,
                            new_pass_opt,
                            old_pass_opt,
                            lvl,
                            Some(progress_cb),
                        )
                        .and_then(|meta| {
                            std::fs::rename(&part_target, &target)?;
                            Ok(meta)
                        })
                        .map_err(|e| e.to_string());

                        let _ = tx.send(WorkerEvent::ArchiveDone(res));
                    });
                }
            }
            AppState::InspectClean => {}
        }
    };

    // Asynchronous Extraction Handler
    let handle_extract_action = move |_| {
        if *is_processing.read() {
            return;
        }

        if *app_state.read() == AppState::InspectDirty {
            show_dirty_banner.set(true);
            return;
        }

        let Some(target) = archive_path.read().clone() else {
            status_message.set("No archive opened to extract".to_string());
            return;
        };

        if let Some(dest) = rfd::FileDialog::new().pick_folder() {
            let pass = if *is_encrypted.read() { passphrase.read().clone() } else { String::new() };

            is_processing.set(true);
            is_extracting.set(true);
            show_graph.set(true);
            progress_pct.set(0.0);
            current_speed_mb.set(0.0);
            peak_speed_mb.set(0.0);
            speed_samples.write().clear();
            speed_samples.write().extend(vec![0.0f64; 60]);
            status_message.set("Extracting archive...".to_string());

            let tx = worker_tx.read().clone();
            let tx_prog = tx.clone();
            let progress_cb = Arc::new(move |p: ArchiveProgress| {
                let _ = tx_prog.send(WorkerEvent::Progress(p));
            });

            let dest_clone = dest.clone();
            std::thread::spawn(move || {
                let pass_opt = if pass.is_empty() { None } else { Some(pass.as_str()) };
                let res = extract_archive(&target, &dest_clone, pass_opt, Some(progress_cb))
                    .map(|_| dest_clone)
                    .map_err(|e| e.to_string());
                let _ = tx.send(WorkerEvent::ExtractDone(res));
            });
        }
    };

    // Speed Graph SVG generation
    let graph_points: String = {
        let samples = speed_samples.read();
        let max_speed = peak_speed_mb.read().max(1.0);
        let width = 300.0f64;
        let height = 64.0f64;
        let step = width / 59.0f64;

        let mut pts = String::new();
        for (i, &s) in samples.iter().enumerate() {
            let x = i as f64 * step;
            let norm = (s / max_speed).clamp(0.0, 1.0);
            let y = height - (norm * (height - 6.0)) - 2.0;
            if i > 0 {
                pts.push(' ');
            }
            pts.push_str(&format!("{:.1},{:.1}", x, y));
        }
        pts
    };

    let graph_polygon: String = format!("0,64 {} 300,64", graph_points);
    let poly_fill = if *is_extracting.read() { "rgba(111, 158, 148, 0.30)" } else { "rgba(194, 113, 77, 0.30)" };
    let stroke_color = if *is_extracting.read() { "#8fb8ae" } else { "#d68b67" };

    let svg_inner = format!(
        "<svg class=\"graph-svg\" viewBox=\"0 0 300 64\" preserveAspectRatio=\"none\"><polygon points=\"{}\" fill=\"{}\"/><polyline points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2\"/></svg>",
        graph_polygon, poly_fill, graph_points, stroke_color
    );

    // Context menu derived info
    let ctx_info = ctx_menu.read().clone().map(|(x, y, row)| {
        let t = tree.read();
        let prefix = format!("{}/", row.path);
        let in_archive: u64 = t
            .committed_entries
            .iter()
            .filter(|e| !t.deleted_paths.contains(&e.path) && (e.path == row.path || e.path.starts_with(&prefix)))
            .map(|e| e.uncompressed_size)
            .sum();
        let entry_count = t
            .committed_entries
            .iter()
            .filter(|e| !e.is_dir && !t.deleted_paths.contains(&e.path) && (e.path == row.path || e.path.starts_with(&prefix)))
            .count();
        let staged: u64 = t
            .staged_items
            .iter()
            .filter(|(_, p, _, d)| !*d && (*p == row.path || p.starts_with(&prefix)))
            .map(|(_, _, s, _)| *s)
            .sum();
        let committed_total: u64 = t.committed_entries.iter().map(|e| e.uncompressed_size).sum();
        let est_packed = if committed_total > 0 {
            (in_archive as f64 * (*committed_compressed_size.read() as f64 / committed_total as f64)) as u64
        } else {
            0
        };
        let can_extract = in_archive > 0 || (row.is_dir && entry_count == 0 && row.status == NodeStatus::Committed);
        let can_extract = can_extract && *app_state.read() != AppState::Staging && !*is_processing.read();
        // Keep menu inside the window
        let win: dioxus::desktop::LogicalSize<f64> =
            desktop.inner_size().to_logical(desktop.scale_factor());
        let mx = x.min(win.width - 250.0).max(0.0);
        let my = y.min(win.height - 220.0).max(0.0);
        (mx, my, row, in_archive, est_packed, staged, entry_count, can_extract)
    });

    let mut ctx_extract = move |_| {
        let Some((_, _, row)) = ctx_menu.read().clone() else { return };
        ctx_menu.set(None);
        let Some(dest) = rfd::FileDialog::new().set_title("Extract to…").pick_folder() else { return };
        let Some(target) = archive_path.read().clone() else { return };
        let pass = if *is_encrypted.read() { passphrase.read().clone() } else { String::new() };
        let tx = worker_tx.read().clone();
        status_message.set(format!("Extracting {}…", row.path));
        std::thread::spawn(move || {
            let pass_opt = if pass.is_empty() { None } else { Some(pass.as_str()) };
            let res = extract_entry(&target, &dest, pass_opt, &row.path)
                .map(|_| dest.join(&row.path))
                .map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::ExtractDone(res));
        });
    };

    let ctx_delete = {
        let mut checker = check_dirty.clone();
        move |_| {
            let Some((_, _, row)) = ctx_menu.read().clone() else { return };
            ctx_menu.set(None);
            tree.write().mark_deleted(&row.path);
            checker();
            update_estimate();
            status_message.set(format!("Removed {}", row.path));
        }
    };

    // Bridge dropped paths from custom protocol and native GTK Wayland handler into App state
    {
        let mut stager = stage_paths.clone();
        let desktop_gtk = desktop.clone();
        use_hook(move || {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
            if let Ok(mut guard) = DROP_TX.lock() {
                *guard = Some(tx.clone());
            }

            // Native GTK3 / WebKitGTK Wayland drag-and-drop handler (fixes wry < 0.47 Bug #1408)
            // On Wayland, GDK emits `drag-leave` (with timestamp != 0) immediately before `drag-drop`.
            // We track the Entered -> Leaving -> Left state machine using `glib::idle_add_local_once`
            // so `drag-drop` can still consume the preloaded URI paths from `drag-data-received`.
            #[cfg(target_os = "linux")]
            {
                use dioxus::desktop::wry::WebViewExtUnix;
                use gtk::prelude::*;
                use std::cell::{Cell, RefCell};
                use std::rc::Rc;

                #[derive(Clone, Copy, Debug, PartialEq, Eq)]
                enum GtkDragState {
                    Left,
                    Entered,
                    Leaving,
                }

                let gtk_webview = desktop_gtk.webview.webview();
                println!("[DND-GTK] Attaching native GTK3/Wayland drag signals to WebKitWebView");

                let cached_paths: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));
                let drag_state: Rc<Cell<GtkDragState>> = Rc::new(Cell::new(GtkDragState::Left));
                let pending_drop: Rc<Cell<bool>> = Rc::new(Cell::new(false));

                // 1. drag-data-received: WebKitGTK's DropTarget requests `text/uri-list` (info == 2)
                // during drag-motion when a file enters the window.
                {
                    let cached_paths = cached_paths.clone();
                    let drag_state = drag_state.clone();
                    let pending_drop = pending_drop.clone();
                    let tx_data = tx.clone();
                    gtk_webview.connect_drag_data_received(move |_w, _ctx, x, y, data, info, time| {
                        let mut is_dragging_sig = is_dragging;
                        let uris = data.uris();
                        let text = data.text().map(|s| s.to_string());
                        println!(
                            "[DND-GTK] drag-data-received at ({}, {}) info={} time={} uris={:?} text={:?}",
                            x, y, info, time, uris, text
                        );

                        let mut paths: Vec<PathBuf> = uris
                            .iter()
                            .map(|u| clean_path(std::path::Path::new(u.as_str())))
                            .filter(|p| !p.as_os_str().is_empty())
                            .collect();

                        if paths.is_empty() {
                            if let Some(ref t) = text {
                                for line in t.lines() {
                                    let l = line.trim();
                                    if !l.is_empty() && !l.starts_with('#') {
                                        let p = clean_path(std::path::Path::new(l));
                                        if p.exists() {
                                            paths.push(p);
                                        }
                                    }
                                }
                            }
                        }

                        if !paths.is_empty() {
                            if pending_drop.get() {
                                println!(
                                    "[DND-GTK] Fulfilling pending drop from drag-data-received with {} path(s): {:?}",
                                    paths.len(),
                                    paths
                                );
                                pending_drop.set(false);
                                drag_state.set(GtkDragState::Left);
                                is_dragging_sig.set(false);
                                let _ = tx_data.send(paths);
                            } else {
                                println!("[DND-GTK] Cached {} path(s) from drag-data-received: {:?}", paths.len(), paths);
                                *cached_paths.borrow_mut() = paths;
                                drag_state.set(GtkDragState::Entered);
                                is_dragging_sig.set(true);
                            }
                        }
                    });
                }

                // 2. drag-motion: fired while dragging over the webview
                {
                    let drag_state = drag_state.clone();
                    let cached_paths = cached_paths.clone();
                    let pending_drop = pending_drop.clone();
                    gtk_webview.connect_drag_motion(move |w, ctx, x, y, time| {
                        let mut is_dragging_sig = is_dragging;
                        if drag_state.get() == GtkDragState::Left {
                            pending_drop.set(false);
                            let targets = ctx.list_targets();
                            let target_names: Vec<String> = targets.iter().map(|a| a.name().to_string()).collect();
                            println!(
                                "[DND-GTK] drag-motion initial at ({}, {}) time={} targets={:?}",
                                x, y, time, target_names
                            );
                            drag_state.set(GtkDragState::Entered);
                            is_dragging_sig.set(true);

                            // Explicitly request text/uri-list if not yet populated
                            if cached_paths.borrow().is_empty() {
                                let uri_atom = gtk::gdk::Atom::intern("text/uri-list");
                                if targets.iter().any(|a| *a == uri_atom) {
                                    println!("[DND-GTK] Requesting text/uri-list via gtk_drag_get_data");
                                    w.drag_get_data(ctx, &uri_atom, time);
                                }
                            }
                        } else if drag_state.get() == GtkDragState::Leaving {
                            drag_state.set(GtkDragState::Entered);
                        }
                        false // let WebKitGTK also run its handler
                    });
                }

                // 3. drag-leave: On Wayland, GTK fires `drag-leave` right before `drag-drop`!
                // Defer the transition to `Left` via `idle_add_local_once` so `drag-drop` sees `Leaving`.
                {
                    let drag_state = drag_state.clone();
                    gtk_webview.connect_drag_leave(move |_w, _ctx, time| {
                        println!("[DND-GTK] drag-leave time={} current_state={:?}", time, drag_state.get());
                        if drag_state.get() != GtkDragState::Left {
                            drag_state.set(GtkDragState::Leaving);
                            let drag_state_idle = drag_state.clone();
                            gtk::glib::idle_add_local_once(move || {
                                let mut is_dragging_sig = is_dragging;
                                if drag_state_idle.get() == GtkDragState::Leaving {
                                    println!("[DND-GTK] drag-leave confirmed (no drop occurred)");
                                    drag_state_idle.set(GtkDragState::Left);
                                    is_dragging_sig.set(false);
                                }
                            });
                        }
                    });
                }

                // 4. drag-drop: fired when user releases mouse button to drop
                {
                    let drag_state = drag_state.clone();
                    let cached_paths = cached_paths.clone();
                    let pending_drop = pending_drop.clone();
                    let tx_drop = tx.clone();
                    gtk_webview.connect_drag_drop(move |w, ctx, x, y, time| {
                        let mut is_dragging_sig = is_dragging;
                        println!(
                            "[DND-GTK] drag-drop at ({}, {}) time={} state={:?} cached_count={}",
                            x,
                            y,
                            time,
                            drag_state.get(),
                            cached_paths.borrow().len()
                        );
                        drag_state.set(GtkDragState::Left);
                        is_dragging_sig.set(false);

                        let paths: Vec<PathBuf> = cached_paths.borrow_mut().drain(..).collect();
                        if !paths.is_empty() {
                            pending_drop.set(false);
                            println!("[DND-GTK] Dispatching {} dropped path(s) to channel: {:?}", paths.len(), paths);
                            let _ = tx_drop.send(paths);
                        } else {
                            println!("[DND-GTK] cached_paths empty on drop, requesting text/uri-list with pending_drop=true");
                            pending_drop.set(true);
                            let uri_atom = gtk::gdk::Atom::intern("text/uri-list");
                            w.drag_get_data(ctx, &uri_atom, time);
                        }
                        false // allow WebKitGTK's handler to finish the GdkDragContext
                    });
                }
            }

            spawn(async move {
                // Deduplicate rapid drops from multiple bridges within 250ms
                let mut last_drop: Option<(std::time::Instant, Vec<PathBuf>)> = None;
                while let Some(paths) = rx.recv().await {
                    println!("[DND-CHANNEL] Received {} path(s): {:?}", paths.len(), paths);
                    is_dragging.set(false);
                    if !paths.is_empty() {
                        let now = std::time::Instant::now();
                        if let Some((prev_t, ref prev_p)) = last_drop {
                            if now.duration_since(prev_t).as_millis() < 250 && *prev_p == paths {
                                println!("[DND-CHANNEL] Skipping duplicate drop event within 250ms");
                                continue;
                            }
                        }
                        last_drop = Some((now, paths.clone()));
                        let count = paths.len();
                        stager(paths);
                        status_message.set(format!("Staged {} item(s)", count));
                    }
                }
            });
        });
    }

    // Secondary bridge using long-lived document::eval Promise + Dioxus #1504 workaround
    {
        let mut stager = stage_paths.clone();
        use_hook(move || {
            spawn(async move {
                let mut eval = document::eval(r#"
                    // Dioxus #1504 / WebKit Bug 265857 workaround:
                    // Attach dragstart setData listener to all draggable elements
                    let draggableElements = document.querySelectorAll('[draggable="true"]');
                    draggableElements.forEach((el) => {
                        el.addEventListener("dragstart", (ev) => {
                            ev.dataTransfer.setData("text/plain", ev.target.id || "draggable");
                        });
                    });

                    window.__kallara_log = function(msg) {
                        dioxus.send({ type: 'log', msg: msg });
                    };
                    window.__kallara_send_drop = function(paths) {
                        dioxus.send({ type: 'drop', paths: paths });
                    };
                    window.__kallara_signal_drag = function(active) {
                        dioxus.send({ type: active ? 'dragenter' : 'dragleave' });
                    };
                    return new Promise(() => {});
                "#);

                while let Ok(msg) = eval.recv::<serde_json::Value>().await {
                    if let Some(t) = msg.get("type").and_then(|v| v.as_str()) {
                        match t {
                            "log" => {
                                if let Some(m) = msg.get("msg").and_then(|v| v.as_str()) {
                                    println!("{}", m);
                                }
                            }
                            "dragenter" => {
                                println!("[DND-EVAL] Received dragenter signal");
                                is_dragging.set(true);
                            }
                            "dragleave" => {
                                println!("[DND-EVAL] Received dragleave signal");
                                is_dragging.set(false);
                            }
                            "drop" => {
                                is_dragging.set(false);
                                if let Some(arr) = msg.get("paths").and_then(|v| v.as_array()) {
                                    let paths: Vec<PathBuf> = arr
                                        .iter()
                                        .filter_map(|v| v.as_str())
                                        .map(PathBuf::from)
                                        .collect();
                                    println!("[DND-EVAL] Received drop with paths: {:?}", paths);
                                    if !paths.is_empty() {
                                        if let Ok(guard) = DROP_TX.lock() {
                                            if let Some(ref tx) = *guard {
                                                let _ = tx.send(paths);
                                                continue;
                                            }
                                        }
                                        stager(paths);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            });
        });
    }

    let key_input_class: &'static str = if *key_pulsing.read() {
        "sunken-input pulsing"
    } else if !*is_encrypted.read() {
        "sunken-input disabled"
    } else {
        "sunken-input"
    };

    let pass_placeholder = if *is_encrypted.read() { "Enter password" } else { "(encryption off)" };
    let progress_fill_class = if *is_extracting.read() { "progress-fill extract" } else { "progress-fill" };
    let progress_width = if *is_processing.read() {
        format!("width: {:.1}%;", *progress_pct.read())
    } else {
        "width: 100%;".to_string()
    };
    let progress_label_text = if *is_processing.read() {
        format!("{:.0}% — {:.1} MB/s", *progress_pct.read(), *current_speed_mb.read())
    } else {
        "100% — Complete".to_string()
    };
    let graph_overlay_label = if *is_processing.read() {
        format!("{:.1} MB/s [peak: {:.1} MB/s]", *current_speed_mb.read(), *peak_speed_mb.read())
    } else {
        format!("Finished [peak: {:.1} MB/s]", *peak_speed_mb.read())
    };

    let mut stage_paths_app = stage_paths.clone();
    let mut stage_paths_well = stage_paths.clone();

    let dirty_banner_text = {
        let staged_count = tree.read().staged_items.len();
        let deleted_count = tree.read().deleted_paths.len();
        if staged_count > 0 && deleted_count > 0 {
            format!("UNARCHIVED CHANGES (+{} / -{} FILES).", staged_count, deleted_count)
        } else if staged_count > 0 {
            format!("UNARCHIVED CHANGES (+{} FILES).", staged_count)
        } else if deleted_count > 0 {
            format!("UNARCHIVED CHANGES (-{} FILES).", deleted_count)
        } else {
            "UNARCHIVED SETTINGS CHANGES (KEY / LVL).".to_string()
        }
    };

    rsx! {
        div {
            class: if *is_dragging.read() {
                if *is_maximized.read() { "app-container drag-active maximized" } else { "app-container drag-active" }
            } else {
                if *is_maximized.read() { "app-container maximized" } else { "app-container" }
            },
            onmouseup: move |_| {
                is_dragging.set(false);
            },
            ondragenter: move |e| {
                println!("[DND-DIOXUS] app-container ondragenter fired");
                e.prevent_default();
                is_dragging.set(true);
            },
            ondragover: move |e| {
                e.prevent_default();
                if !*is_dragging.read() {
                    println!("[DND-DIOXUS] app-container ondragover activating is_dragging");
                    is_dragging.set(true);
                }
            },
            ondragleave: move |e| {
                println!("[DND-DIOXUS] app-container ondragleave fired");
                e.prevent_default();
                is_dragging.set(false);
            },
            ondrop: move |e: DragEvent| {
                println!("[DND-DIOXUS] app-container ondrop fired, has_files={}", e.files().is_some());
                e.prevent_default();
                is_dragging.set(false);
                if let Some(engine) = e.files() {
                    let paths: Vec<PathBuf> = engine.files().into_iter().map(PathBuf::from).collect();
                    println!("[DND-DIOXUS] app-container e.files() paths={:?}", paths);
                    if !paths.is_empty() {
                        let count = paths.len();
                        stage_paths_app(paths);
                        status_message.set(format!("Staged {} item(s)", count));
                    }
                }
            },

            // Edge and corner resize handles (only active when not maximized)
            if !*is_maximized.read() {
                div {
                    class: "resize-handle resize-n",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::North); }
                    }
                }
                div {
                    class: "resize-handle resize-s",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::South); }
                    }
                }
                div {
                    class: "resize-handle resize-e",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::East); }
                    }
                }
                div {
                    class: "resize-handle resize-w",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::West); }
                    }
                }
                div {
                    class: "resize-handle resize-nw",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::NorthWest); }
                    }
                }
                div {
                    class: "resize-handle resize-ne",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::NorthEast); }
                    }
                }
                div {
                    class: "resize-handle resize-sw",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::SouthWest); }
                    }
                }
                div {
                    class: "resize-handle resize-se",
                    onmousedown: {
                        let d = desktop.clone();
                        move |_| { let _ = d.drag_resize_window(ResizeDirection::SouthEast); }
                    }
                }
            }

            // Custom Window Title Bar
            div {
                class: "title-bar",
                onmousedown: {
                    let d = desktop.clone();
                    move |e: MouseEvent| {
                        if e.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
                            d.drag();
                        }
                    }
                },
                ondoubleclick: {
                    let d = desktop.clone();
                    move |_| {
                        d.toggle_maximized();
                        is_maximized.set(d.is_maximized());
                    }
                },
                div { class: "title-bar-left",
                    span { class: "title-bar-icon", "KL" }
                    span { class: "title-bar-title", "KALLARA — High-Density Archive Engine" }
                }
                div { class: "title-bar-status", "{status_message.read()}" }
                div { class: "window-controls",
                    button {
                        class: "window-control-btn",
                        title: "Minimize",
                        onmousedown: move |e: MouseEvent| e.stop_propagation(),
                        onclick: handle_minimize,
                        "−"
                    }
                    button {
                        class: "window-control-btn",
                        title: if *is_maximized.read() { "Restore" } else { "Maximize" },
                        onmousedown: move |e: MouseEvent| e.stop_propagation(),
                        onclick: handle_maximize,
                        if *is_maximized.read() { "❐" } else { "□" }
                    }
                    button {
                        class: "window-control-btn close-btn",
                        title: "Close",
                        onmousedown: move |e: MouseEvent| e.stop_propagation(),
                        onclick: handle_close,
                        "✕"
                    }
                }
            }

            // Main Split Pane
            div { class: "main-content",
                // Left Pane: Sunken Tree View (Drag and Drop Zone)
                div { class: "left-pane",
                    div { class: "tree-header",
                        span { "{tree_header_title}" }
                        div { style: "display: flex; gap: 4px;",
                            button {
                                class: "raised-button secondary-button",
                                onclick: handle_stage_files,
                                "+ Files"
                            }
                            button {
                                class: "raised-button secondary-button",
                                onclick: handle_stage_folder,
                                "+ Folder"
                            }
                            button {
                                class: "raised-button secondary-button",
                                onclick: handle_open_archive,
                                "Open .klr"
                            }
                        }
                    }

                    // Sunken Virtualized Tree Container
                    div {
                        class: if *is_dragging.read() { "tree-well drag-over" } else { "tree-well" },
                        onmouseup: move |_| {
                            is_dragging.set(false);
                        },
                        ondragover: move |e| {
                            e.prevent_default();
                            if !*is_dragging.read() {
                                println!("[DND-DIOXUS] tree-well ondragover activating is_dragging");
                                is_dragging.set(true);
                            }
                        },
                        ondrop: move |e: DragEvent| {
                            println!("[DND-DIOXUS] tree-well ondrop fired, has_files={}", e.files().is_some());
                            e.prevent_default();
                            is_dragging.set(false);
                            if let Some(engine) = e.files() {
                                let paths: Vec<PathBuf> = engine.files().into_iter().map(PathBuf::from).collect();
                                println!("[DND-DIOXUS] tree-well e.files() paths={:?}", paths);
                                if !paths.is_empty() {
                                    stage_paths_well(paths);
                                }
                            }
                        },
                        onmounted: move |e| {
                            let m = e.data();
                            tree_mount.set(Some(m.clone()));
                            spawn(async move {
                                if let Ok(r) = m.get_client_rect().await {
                                    viewport_h.set(r.size.height.max(200.0));
                                }
                            });
                        },
                        onresize: move |e| {
                            if let Ok(sz) = e.get_content_box_size() {
                                viewport_h.set(sz.height.max(200.0));
                            }
                        },
                        onscroll: move |_| {
                            if let Some(m) = tree_mount.read().clone() {
                                spawn(async move {
                                    if let Ok(off) = m.get_scroll_offset().await {
                                        scroll_top.set(off.y.max(0.0));
                                    }
                                });
                            }
                        },
                        if *is_dragging.read() {
                            div { class: "drop-overlay", "[ + DROP FILES OR FOLDERS TO STAGE ]" }
                        }
                        if total_rows == 0 && !*is_dragging.read() {
                            div { class: "empty-hint", "drop files · folders · .klr" }
                        }
                        div {
                            style: "height: {total_rows as f64 * row_height + 4.0}px; position: relative; min-width: 100%; width: max-content;",
                            div {
                                style: "position: absolute; top: {start_idx as f64 * row_height}px; left: 0; right: 0;",
                                for row in visible_rows {
                                    div {
                                        key: "{row.id}",
                                        class: row_class(row.status == NodeStatus::Staged, tree.read().selected_id.as_deref() == Some(&row.id)),
                                        onclick: {
                                            let rid = row.id.clone();
                                            let is_dir = row.is_dir;
                                            move |_| {
                                                tree.write().selected_id = Some(rid.clone());
                                                if is_dir {
                                                    tree.write().toggle_expand(&rid);
                                                }
                                            }
                                        },
                                        oncontextmenu: {
                                            let r = row.clone();
                                            move |e: MouseEvent| {
                                                e.prevent_default();
                                                let p = e.client_coordinates();
                                                tree.write().selected_id = Some(r.id.clone());
                                                ctx_menu.set(Some((p.x, p.y, r.clone())));
                                            }
                                        },
                                        span { class: "tree-guide", "{row.prefix}" }
                                        span {
                                            class: glyph_class(row.status == NodeStatus::Staged, row.is_dir),
                                            if row.status == NodeStatus::Staged {
                                                "[+]"
                                            } else if row.is_dir {
                                                if row.expanded { "[-]" } else { "[D]" }
                                            } else {
                                                "[F]"
                                            }
                                        }
                                        span { class: "tree-name", "{row.name}" }
                                        if !row.is_dir {
                                            span { class: "tree-size", "{format_bytes(row.size)}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Right-click context menu
                if let Some((mx, my, row, in_archive, est_packed, staged, entry_count, can_extract)) = ctx_info.clone() {
                    div {
                        class: "ctx-backdrop",
                        onclick: move |_| ctx_menu.set(None),
                        oncontextmenu: move |e: MouseEvent| { e.prevent_default(); ctx_menu.set(None); },
                    }
                    div {
                        class: "ctx-menu",
                        style: "left: {mx}px; top: {my}px;",
                        oncontextmenu: move |e: MouseEvent| e.prevent_default(),
                        div { class: "ctx-title", if row.is_dir { "[D] " } else { "[F] " } "{row.name}" }
                        div { class: "ctx-info", span { "In archive" } span { class: "v", "{format_bytes(in_archive)}" } }
                        if in_archive > 0 {
                            div { class: "ctx-info", span { "Packed (est.)" } span { class: "v", "~{format_bytes(est_packed)}" } }
                        }
                        if staged > 0 {
                            div { class: "ctx-info", span { "Staged" } span { class: "v", "+{format_bytes(staged)}" } }
                        }
                        if row.is_dir {
                            div { class: "ctx-info", span { "Files" } span { class: "v", "{entry_count}" } }
                        }
                        div { class: "ctx-sep" }
                        div {
                            class: if can_extract { "ctx-item" } else { "ctx-item disabled" },
                            onclick: move |e| { if can_extract { ctx_extract(e) } },
                            "Extract alone…"
                        }
                        div {
                            class: "ctx-item danger",
                            onclick: ctx_delete,
                            "Delete"
                        }
                    }
                }

                // Right Pane: Telemetry & Controls
                div { class: "right-pane",
                    // Telemetry Well
                    div { class: "telemetry-well",
                        div { class: "telemetry-row",
                            span { class: "telemetry-label", "Uncompressed" }
                            span { class: "telemetry-val",
                                "{format_bytes(committed_uncompressed)} "
                                if uncompressed_delta != 0 {
                                    span { class: "telemetry-val delta", "{format_bytes_delta(uncompressed_delta)}" }
                                }
                            }
                        }
                        div { class: "telemetry-row",
                            span { class: "telemetry-label", "Compressed" }
                            span { class: "telemetry-val", "{compressed_display_str}" }
                        }
                        div { class: "telemetry-row",
                            span { class: "telemetry-label", "Ratio" }
                            span { class: "telemetry-val", "{ratio_str}" }
                        }
                        div { class: "telemetry-row",
                            span { class: "telemetry-label", "Nodes" }
                            span { class: "telemetry-val", "{file_count}/{dir_count}" }
                        }
                    }

                    // Control Strip (Columnar: Encryption Checkbox, Key Input, Compression Level)
                    div { class: "control-strip",
                        div { class: "control-row",
                            label {
                                class: "encrypt-checkbox-label",
                                input {
                                    class: "tactile-checkbox",
                                    r#type: "checkbox",
                                    checked: *is_encrypted.read(),
                                    onchange: {
                                        let mut checker = check_dirty.clone();
                                        move |e: FormEvent| {
                                            let checked = e.checked();
                                            is_encrypted.set(checked);
                                            if !checked {
                                                key_pulsing.set(false);
                                            }
                                            checker();
                                        }
                                    }
                                }
                                span { class: "input-label", "ENCRYPT" }
                            }
                        }
                        div { class: "control-row key-input-container",
                            span { class: "input-label", "KEY" }
                            input {
                                class: key_input_class,
                                style: "flex-grow: 1;",
                                r#type: "password",
                                disabled: !*is_encrypted.read(),
                                placeholder: pass_placeholder,
                                value: "{passphrase.read()}",
                                oninput: {
                                    let mut checker = check_dirty.clone();
                                    move |e: FormEvent| {
                                        passphrase.set(e.value());
                                        key_pulsing.set(false);
                                        checker();
                                    }
                                },
                                onkeydown: {
                                    let mut opener = open_klr_archive.clone();
                                    move |e: KeyboardEvent| {
                                        if e.key() == Key::Enter {
                                            if let Some(ref path) = *archive_path.read() {
                                                if tree.read().committed_entries.is_empty() && path.exists() {
                                                    opener(path.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "control-row lvl-input-container",
                            span { class: "input-label", "LVL" }
                            input {
                                class: "sunken-input lvl-stepper",
                                r#type: "number",
                                min: "1",
                                max: "19",
                                value: "{zstd_level.read()}",
                                oninput: {
                                    let mut checker = check_dirty.clone();
                                    move |e: FormEvent| {
                                        if let Ok(lvl) = e.value().parse::<u8>() {
                                            zstd_level.set(lvl.clamp(1, 19));
                                            update_estimate();
                                            checker();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Progress Bar & Speed Graph (visible during active archival/extraction and after completion)
                    if *is_processing.read() || *show_graph.read() {
                        div { class: "active-archival-block",
                            // Tactile Progress Bar
                            div { class: "progress-well",
                                div {
                                    class: progress_fill_class,
                                    style: "{progress_width}"
                                }
                                div { class: "progress-overlay-label",
                                    "{progress_label_text}"
                                }
                            }
                            // Speed Graph Curve
                            div { class: "graph-well",
                                div {
                                    style: "width: 100%; height: 100%;",
                                    dangerous_inner_html: "{svg_inner}"
                                }
                                div { class: "graph-overlay-text",
                                    span { "{graph_overlay_label}" }
                                    if !*is_processing.read() {
                                        button {
                                            class: "graph-close-btn",
                                            title: "Dismiss graph",
                                            onclick: move |_| show_graph.set(false),
                                            "×"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "spacer" }

                    // Exit / Overwrite Guard High-Contrast Banner
                    if *show_dirty_banner.read() {
                        div { class: "dirty-banner",
                            span { "{dirty_banner_text}" }
                            div { class: "dirty-banner-actions",
                                button {
                                    class: "raised-button secondary-button",
                                    onclick: handle_archive_action,
                                    "ARCHIVE NOW"
                                }
                                button {
                                    class: "raised-button secondary-button",
                                    onclick: move |_| {
                                        tree.write().staged_items.clear();
                                        tree.write().deleted_paths.clear();
                                        if let Some(enc) = *baseline_encrypted.read() {
                                            is_encrypted.set(enc);
                                        }
                                        if let Some(ref pass) = *baseline_passphrase.read() {
                                            passphrase.set(pass.clone());
                                        } else {
                                            passphrase.set(String::new());
                                        }
                                        if let Some(lvl) = *baseline_zstd_level.read() {
                                            zstd_level.set(lvl);
                                        }
                                        if let Some(ref p) = *opened_archive_path.read() {
                                            archive_path.set(Some(p.clone()));
                                        }
                                        key_pulsing.set(false);
                                        app_state.set(AppState::InspectClean);
                                        show_dirty_banner.set(false);
                                        status_message.set("Changes discarded".to_string());
                                    },
                                    "DISCARD"
                                }
                            }
                        }
                    }

                    // Primary Action Button Block
                    div { class: "{action_grid_class}",
                        button {
                            class: "{archive_btn_class}",
                            disabled: is_archive_btn_disabled,
                            onclick: handle_archive_action,
                            span { "archive" }
                            if *app_state.read() == AppState::InspectDirty {
                                span { "*" }
                            }
                            sub { "verb" }
                        }

                        if *app_state.read() != AppState::Staging {
                            button {
                                class: "raised-button secondary",
                                disabled: *is_processing.read(),
                                onclick: handle_extract_action,
                                span { "extract" }
                                sub { "verb" }
                            }
                        }
                    }

                    // Path Footer
                    div { class: "path-footer",
                        span {
                            class: "path-text",
                            title: archive_path.read().as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                            if let Some(ref p) = *archive_path.read() {
                                "{p.display()}"
                            } else {
                                "[No location selected]"
                            }
                        }
                        span {
                            class: "change-link",
                            onclick: {
                                let mut checker = check_dirty.clone();
                                move |_| {
                                    let mut dialog = rfd::FileDialog::new().add_filter("Kallara Archive", &["klr"]);
                                    if let Some(ref current) = *archive_path.read() {
                                        if let Some(parent) = current.parent() {
                                            dialog = dialog.set_directory(parent);
                                        }
                                        if let Some(name) = current.file_name() {
                                            dialog = dialog.set_file_name(name.to_string_lossy().to_string());
                                        }
                                    } else {
                                        dialog = dialog.set_file_name("archive.klr");
                                    }
                                    if let Some(new_path) = dialog.save_file() {
                                        let final_path = if new_path.extension().is_none() {
                                            new_path.with_extension("klr")
                                        } else {
                                            new_path
                                        };
                                        archive_path.set(Some(final_path));
                                        checker();
                                    }
                                }
                            },
                            if archive_path.read().is_some() { "[change location]" } else { "[set location]" }
                        }
                    }
                }
            }
        }
    }
}
