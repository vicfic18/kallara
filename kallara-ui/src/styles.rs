use crate::assets::{REDACTION35_BOLD_B64, REDACTION35_REGULAR_B64};

/// Logo (black on alpha) embedded at compile time; rendered as an inverted faint watermark.
const LOGO_PNG: &[u8] = include_bytes!("../assets/kallara-logo.png");

fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn generate_css() -> String {
    format!(
        r#"
:root {{
    /* Primary: terracotta */
    --p-100: #f3d6c8;
    --p-200: #e6a888;
    --p-300: #d68b67;
    --p-400: #c2714d;
    --p-500: #a85c3b;
    --p-600: #84472d;
    --p-700: #5e3322;

    /* Secondary: sage teal */
    --s-200: #b5d1cb;
    --s-300: #8fb8ae;
    --s-400: #6f9e94;
    --s-500: #557f76;
    --s-600: #3e5f58;

    /* Dark greys (no pure black) */
    --g-950: #1c1d20;
    --g-900: #232428;
    --g-850: #292a2f;
    --g-800: #303136;
    --g-700: #3a3b41;
    --g-600: #46474e;
    --g-500: #55565e;
    --g-400: #6b6c74;
    --g-300: #8c8d95;
    --g-200: #b4b2ae;
    --g-100: #e4e0da;

    /* Bevel primitives */
    --hl: var(--g-500);
    --hl-inner: var(--g-600);
    --sh: var(--g-950);
    --sh-inner: var(--g-900);
}}

@font-face {{
    font-family: 'Redaction 35';
    src: url('data:font/woff2;base64,{reg}') format('woff2');
    font-weight: normal;
}}
@font-face {{
    font-family: 'Redaction 35';
    src: url('data:font/woff2;base64,{bold}') format('woff2');
    font-weight: bold;
}}

* {{
    box-sizing: border-box;
    margin: 0;
    padding: 0;
    user-select: none;
    -webkit-user-select: none;
}}

html, body {{
    width: 100vw;
    height: 100vh;
    background: var(--g-800);
    color: var(--g-100);
    font-family: 'Redaction 35', 'Segoe UI', system-ui, sans-serif;
    font-size: 15px;
    line-height: 1.25;
    overflow: hidden;
}}

.mono {{ font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace; }}

body.drag-active, body.drag-active *,
.app-container.drag-active, .app-container.drag-active * {{
    cursor: copy !important;
}}

::-webkit-scrollbar {{ width: 16px; height: 16px; background: var(--g-850); }}
::-webkit-scrollbar-track {{
    background: repeating-conic-gradient(var(--g-850) 0% 25%, var(--g-800) 0% 50%) 0 0 / 2px 2px;
}}
::-webkit-scrollbar-thumb {{
    background: var(--g-700);
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
}}
::-webkit-scrollbar-thumb:hover {{ background: var(--g-600); }}

/* ---------- Bevel primitives ---------- */
.raised {{
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
    box-shadow: inset -1px -1px 0 var(--sh-inner), inset 1px 1px 0 var(--hl-inner);
}}
.sunken {{
    border-top: 2px solid var(--sh-inner);
    border-left: 2px solid var(--sh-inner);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
    box-shadow: inset 1px 1px 0 var(--sh);
}}

/* ---------- Window frame ---------- */
.app-container {{
    position: relative;
    display: flex;
    flex-direction: column;
    width: 100vw;
    height: 100vh;
    padding: 4px;
    gap: 4px;
    background: var(--g-800);
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
}}
.app-container.maximized {{ border-width: 0; }}

.title-bar {{
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    height: 32px;
    min-height: 32px;
    padding: 2px 4px 2px 8px;
    background: linear-gradient(90deg, var(--p-600) 0%, var(--p-500) 45%, var(--g-700) 100%);
    color: var(--p-100);
    font-weight: bold;
    cursor: default;
}}
.title-bar-left {{ display: flex; align-items: center; gap: 8px; overflow: hidden; white-space: nowrap; }}
.title-bar-icon {{
    font-size: 11px;
    font-weight: 900;
    padding: 1px 5px;
    background: var(--p-400);
    color: var(--g-900);
    border-top: 1px solid var(--p-200);
    border-left: 1px solid var(--p-200);
    border-right: 1px solid var(--p-700);
    border-bottom: 1px solid var(--p-700);
}}
.title-bar-title {{ font-size: 14px; letter-spacing: 0.5px; }}
.title-bar-status {{
    flex: 1;
    text-align: right;
    font-size: 13px;
    font-weight: normal;
    color: var(--p-100);
    opacity: 0.8;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}}
.window-controls {{ display: flex; gap: 3px; flex-shrink: 0; }}
.window-control-btn {{
    width: 24px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--g-700);
    color: var(--g-100);
    font-size: 13px;
    font-weight: bold;
    cursor: pointer;
    border-top: 1px solid var(--hl);
    border-left: 1px solid var(--hl);
    border-right: 1px solid var(--sh);
    border-bottom: 1px solid var(--sh);
}}
.window-control-btn:hover {{ background: var(--g-600); }}
.window-control-btn:active {{
    border-top: 1px solid var(--sh);
    border-left: 1px solid var(--sh);
    border-right: 1px solid var(--hl);
    border-bottom: 1px solid var(--hl);
}}
.window-control-btn.close-btn:hover {{ background: var(--p-500); color: var(--p-100); }}

.resize-handle {{ position: absolute; z-index: 9999; }}
.resize-n {{ top: 0; left: 8px; right: 8px; height: 4px; cursor: n-resize; }}
.resize-s {{ bottom: 0; left: 8px; right: 8px; height: 4px; cursor: s-resize; }}
.resize-w {{ top: 8px; bottom: 8px; left: 0; width: 4px; cursor: w-resize; }}
.resize-e {{ top: 8px; bottom: 8px; right: 0; width: 4px; cursor: e-resize; }}
.resize-nw {{ top: 0; left: 0; width: 8px; height: 8px; cursor: nw-resize; }}
.resize-ne {{ top: 0; right: 0; width: 8px; height: 8px; cursor: ne-resize; }}
.resize-sw {{ bottom: 0; left: 0; width: 8px; height: 8px; cursor: sw-resize; }}
.resize-se {{ bottom: 0; right: 0; width: 8px; height: 8px; cursor: se-resize; }}

/* ---------- Layout ---------- */
.main-content {{ display: flex; flex: 1; gap: 4px; min-height: 0; }}

.left-pane {{
    width: 60%;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-height: 0;
}}

.tree-header {{
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 3px 4px 3px 8px;
    background: var(--g-700);
    font-weight: bold;
    font-size: 14px;
    color: var(--g-100);
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
}}
.tree-header .state-tag {{ color: var(--s-300); }}
.tree-header .state-tag.dirty {{ color: var(--p-300); }}

.tree-well {{
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: var(--g-900);
    border-top: 2px solid var(--sh-inner);
    border-left: 2px solid var(--sh-inner);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
    box-shadow: inset 1px 1px 0 var(--sh);
}}
.tree-well.drag-over {{ background: var(--g-850); outline: 1px dashed var(--p-400); outline-offset: -6px; }}

.drop-overlay {{
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 18px;
    font-weight: bold;
    color: var(--p-200);
    background: rgba(194, 113, 77, 0.10);
    z-index: 50;
    pointer-events: none;
}}

.empty-hint {{
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--g-400);
    font-size: 14px;
    pointer-events: none;
}}

.tree-row {{
    height: 26px;
    line-height: 26px;
    display: flex;
    align-items: center;
    padding: 0 10px 0 6px;
    white-space: nowrap;
    cursor: default;
    color: var(--g-100);
}}
.tree-row:hover {{ background: var(--g-850); }}
.tree-row.selected {{ background: var(--p-600); color: var(--p-100); }}
.tree-row.staged .tree-name {{ color: var(--s-300); }}
.tree-row.staged.selected .tree-name {{ color: var(--s-200); }}

.tree-guide {{
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    white-space: pre;
    color: var(--g-500);
    font-size: 15px;
}}
.tree-row.selected .tree-guide {{ color: var(--p-300); }}
.tree-glyph {{
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    width: 30px;
    flex-shrink: 0;
    font-size: 13px;
    font-weight: bold;
    color: var(--g-300);
}}
.tree-glyph.dir {{ color: var(--p-300); }}
.tree-glyph.staged {{ color: var(--s-300); }}
.tree-row.selected .tree-glyph {{ color: var(--p-100); }}
.tree-name {{ flex: 1; overflow: hidden; text-overflow: ellipsis; }}
.tree-size {{
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    font-size: 13px;
    color: var(--g-300);
    padding-left: 12px;
}}
.tree-row.selected .tree-size {{ color: var(--p-200); }}

/* ---------- Right pane ---------- */
.right-pane {{
    position: relative;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px;
    overflow-y: auto;
    background: var(--g-800);
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
    isolation: isolate;
}}
.right-pane::before {{
    content: '';
    position: absolute;
    inset: 0;
    background: url('data:image/png;base64,{logo}') no-repeat center 62% / 78% auto;
    filter: invert(1);
    opacity: 0.045;
    pointer-events: none;
    z-index: -1;
}}

.section-label {{
    font-size: 12px;
    font-weight: bold;
    letter-spacing: 1px;
    text-transform: uppercase;
    color: var(--g-300);
    padding-left: 2px;
}}

.telemetry-well {{
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background: rgba(35, 36, 40, 0.85);
    border-top: 2px solid var(--sh-inner);
    border-left: 2px solid var(--sh-inner);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
    box-shadow: inset 1px 1px 0 var(--sh);
}}
.telemetry-row {{ display: flex; justify-content: space-between; align-items: baseline; gap: 8px; }}
.telemetry-label {{ color: var(--g-300); font-size: 13px; letter-spacing: 1px; text-transform: uppercase; }}
.telemetry-val {{
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    font-weight: bold;
    font-size: 14px;
    color: var(--s-200);
}}
.telemetry-val.accent {{ color: var(--p-300); }}
.telemetry-val.delta {{ color: var(--p-300); margin-left: 4px; }}

.control-strip {{
    display: flex;
    flex-direction: column;
    gap: 6px;
}}
.control-row {{
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 28px;
}}
.control-row > .input-label {{
    min-width: 32px;
}}
.encrypt-checkbox-label {{
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    user-select: none;
    -webkit-user-select: none;
    min-height: 28px;
}}
.tactile-checkbox {{
    width: 16px;
    height: 16px;
    accent-color: var(--p-400);
    cursor: pointer;
}}
.key-input-container {{ display: flex; align-items: center; flex: 1; gap: 8px; }}
.input-label {{ font-weight: bold; font-size: 13px; color: var(--g-200); letter-spacing: 1px; }}
.sunken-input {{
    height: 28px;
    padding: 0 8px;
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    font-size: 14px;
    color: var(--g-100);
    background: var(--g-900);
    outline: none;
    border-top: 2px solid var(--sh-inner);
    border-left: 2px solid var(--sh-inner);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
    box-shadow: inset 1px 1px 0 var(--sh);
    user-select: text;
    -webkit-user-select: text;
    transition: opacity 0.15s ease;
    min-width: 0;
}}
.sunken-input:disabled,
.sunken-input.disabled {{
    opacity: 0.45;
    cursor: not-allowed;
    background: var(--g-800);
    color: var(--g-400);
    box-shadow: inset 1px 1px 0 var(--sh);
}}
.sunken-input::placeholder {{ color: var(--g-400); }}
.sunken-input:focus:not(:disabled) {{ box-shadow: inset 1px 1px 0 var(--sh), 0 0 0 1px var(--s-500); }}
.sunken-input.pulsing {{ animation: pulse 0.9s ease-in-out infinite; }}
@keyframes pulse {{
    0%, 100% {{ box-shadow: inset 1px 1px 0 var(--sh), 0 0 0 1px var(--p-600); }}
    50% {{ box-shadow: inset 1px 1px 0 var(--sh), 0 0 0 2px var(--p-300); }}
}}
.lvl-stepper {{ width: 58px; text-align: center; padding: 0 2px; }}

/* Progress + graph */
.active-archival-block {{ display: flex; flex-direction: column; gap: 6px; }}
.progress-well {{
    position: relative;
    height: 24px;
    overflow: hidden;
    background: var(--g-900);
    border-top: 2px solid var(--sh-inner);
    border-left: 2px solid var(--sh-inner);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
}}
.progress-fill {{
    height: 100%;
    background: repeating-linear-gradient(90deg, var(--p-400) 0 10px, transparent 10px 12px);
    transition: width 0.1s linear;
}}
.progress-fill.extract {{
    background: repeating-linear-gradient(90deg, var(--s-400) 0 10px, transparent 10px 12px);
}}
.progress-overlay-label {{
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    font-weight: bold;
    color: var(--g-100);
    text-shadow: 1px 1px 0 var(--g-950);
}}
.graph-well {{
    position: relative;
    height: 72px;
    background-color: var(--g-900);
    background-image:
        linear-gradient(rgba(111, 158, 148, 0.10) 1px, transparent 1px),
        linear-gradient(90deg, rgba(111, 158, 148, 0.10) 1px, transparent 1px);
    background-size: 10px 10px;
    border-top: 2px solid var(--sh-inner);
    border-left: 2px solid var(--sh-inner);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
}}
.graph-svg {{ width: 100%; height: 100%; display: block; }}
.graph-overlay-text {{
    position: absolute;
    top: 3px;
    right: 6px;
    display: inline-flex;
    align-items: center;
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    font-size: 12px;
    color: var(--g-200);
}}
.graph-close-btn {{
    background: transparent;
    border: none;
    color: var(--g-400);
    cursor: pointer;
    font-size: 14px;
    font-weight: bold;
    padding: 0 4px;
    line-height: 1;
    margin-left: 6px;
    transition: color 0.15s ease;
}}
.graph-close-btn:hover {{
    color: var(--p-200);
}}

.spacer {{ flex: 1; }}

.dirty-banner {{
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    font-weight: bold;
    color: var(--p-100);
    background: var(--p-700);
    border: 2px solid var(--p-400);
}}
.dirty-banner-actions {{ display: flex; gap: 6px; }}

/* Buttons */
.raised-button {{
    display: inline-flex;
    align-items: baseline;
    justify-content: center;
    gap: 1px;
    padding: 4px 12px;
    font-family: inherit;
    font-size: 14px;
    font-weight: bold;
    color: var(--g-100);
    background: var(--g-700);
    cursor: pointer;
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
    box-shadow: inset -1px -1px 0 var(--sh-inner), inset 1px 1px 0 var(--hl-inner);
}}
.raised-button:hover:not(:disabled) {{ background: var(--g-600); }}
.raised-button:active:not(:disabled) {{
    border-top: 2px solid var(--sh);
    border-left: 2px solid var(--sh);
    border-right: 2px solid var(--hl);
    border-bottom: 2px solid var(--hl);
    box-shadow: inset 1px 1px 0 var(--sh-inner);
    padding: 5px 11px 3px 13px;
}}
.raised-button:disabled {{ color: var(--g-400); cursor: default; text-shadow: 1px 1px 0 var(--g-850); }}
.secondary-button {{ font-size: 13px; padding: 2px 10px; }}
.secondary-button:active:not(:disabled) {{ padding: 3px 9px 1px 11px; }}

.action-grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }}
.action-grid.single-col {{ grid-template-columns: 1fr; }}
.action-grid .raised-button {{ height: 46px; align-items: center; font-size: 18px; }}
.action-grid .raised-button sub {{ font-size: 11px; align-self: flex-end; margin-bottom: 8px; opacity: 0.75; }}

/* Primary (archive) button */
.raised-button.primary {{
    color: var(--g-950);
    background: var(--p-400);
    border-top-color: var(--p-200);
    border-left-color: var(--p-200);
    border-right-color: var(--p-700);
    border-bottom-color: var(--p-700);
    box-shadow: inset -1px -1px 0 var(--p-600), inset 1px 1px 0 var(--p-300);
    text-shadow: 1px 1px 0 var(--p-600);
}}
.raised-button.primary:hover:not(:disabled) {{ background: var(--p-300); }}
.raised-button.primary:active:not(:disabled) {{
    border-top-color: var(--p-700);
    border-left-color: var(--p-700);
    border-right-color: var(--p-200);
    border-bottom-color: var(--p-200);
    box-shadow: inset 1px 1px 0 var(--p-600);
}}
.raised-button.primary:disabled {{
    color: var(--p-300);
    background: var(--p-700);
    border-top-color: var(--p-600);
    border-left-color: var(--p-600);
    border-right-color: var(--g-950);
    border-bottom-color: var(--g-950);
    box-shadow: none;
    text-shadow: none;
}}
.raised-button.primary.dirty-active {{ animation: dirty-glow 1.6s ease-in-out infinite; }}
@keyframes dirty-glow {{
    0%, 100% {{ outline: 1px solid transparent; }}
    50% {{ outline: 1px dotted var(--p-200); outline-offset: -6px; }}
}}

.raised-button.secondary {{
    color: var(--s-200);
    background: var(--s-600);
    border-top-color: var(--s-400);
    border-left-color: var(--s-400);
    border-right-color: var(--g-950);
    border-bottom-color: var(--g-950);
    box-shadow: inset -1px -1px 0 var(--g-900), inset 1px 1px 0 var(--s-500);
}}
.raised-button.secondary:hover:not(:disabled) {{ background: var(--s-500); }}

.path-footer {{
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--g-300);
}}
.path-text {{
    flex: 1;
    min-width: 0;
    font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
}}
.change-link {{ color: var(--s-300); text-decoration: underline; cursor: pointer; white-space: nowrap; }}
.change-link:hover {{ color: var(--s-200); }}

/* ---------- Context menu ---------- */
.ctx-backdrop {{ position: fixed; inset: 0; z-index: 10000; }}
.ctx-menu {{
    position: fixed;
    min-width: 230px;
    padding: 2px;
    background: var(--g-700);
    z-index: 10001;
    border-top: 2px solid var(--hl);
    border-left: 2px solid var(--hl);
    border-right: 2px solid var(--sh);
    border-bottom: 2px solid var(--sh);
    box-shadow: inset -1px -1px 0 var(--sh-inner), inset 1px 1px 0 var(--hl-inner), 3px 3px 0 rgba(20, 20, 22, 0.5);
}}
.ctx-title {{
    padding: 4px 8px;
    font-weight: bold;
    font-size: 13px;
    color: var(--p-100);
    background: linear-gradient(90deg, var(--p-600), var(--p-500));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 320px;
}}
.ctx-info {{
    display: flex;
    justify-content: space-between;
    gap: 16px;
    padding: 3px 10px;
    font-size: 13px;
    color: var(--g-200);
}}
.ctx-info .v {{ font-family: 'JetBrains Mono', 'DejaVu Sans Mono', monospace; color: var(--s-200); }}
.ctx-sep {{ height: 0; margin: 3px 2px; border-top: 1px solid var(--sh); border-bottom: 1px solid var(--hl); }}
.ctx-item {{ padding: 4px 10px 4px 22px; font-size: 14px; cursor: default; color: var(--g-100); }}
.ctx-item:hover {{ background: var(--p-500); color: var(--p-100); }}
.ctx-item.disabled {{ color: var(--g-400); }}
.ctx-item.disabled:hover {{ background: transparent; color: var(--g-400); }}
.ctx-item.danger:hover {{ background: var(--p-700); }}
"#,
        reg = REDACTION35_REGULAR_B64,
        bold = REDACTION35_BOLD_B64,
        logo = base64_encode(LOGO_PNG),
    )
}
