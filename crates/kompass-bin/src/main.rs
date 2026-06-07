//! Kompass — desktop app.
//!
//! Implements the Claude Design system (Rose mark + indigo accent, IBM Plex
//! Sans + JetBrains Mono, dual theme) on top of the kompass-core engine.
//! All cluster I/O runs on a background tokio runtime; the Dioxus UI thread
//! only consumes deltas over a channel and renders from in-memory state
//! (ARCHITECTURE.md §1, §12).
//!
//! This build ships the app shell (cluster hairline, topbar, left nav) and the
//! workhorse Resource list (Pods) with live freshness states, wired to real
//! cluster data. Overview / Secrets / Detail panel / ⌘K palette are next.

mod config;

use dioxus::desktop::tao::platform::macos::WindowBuilderExtMacOS;
use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use kompass_core::{
    cluster_accent_index, columns_for, container_states_from_yaml, has_logs, has_metrics,
    is_data_kind, is_workload, list_contexts, run_engine, Cmd, ConnState, ContainerState, Delta,
    EventRow, KindMeta, OverviewData, PortForward, ResourceRow,
};
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use tokio::sync::mpsc::unbounded_channel;

/// Design tokens + app-shell styles, ported verbatim from the Claude Design
/// bundle (the CSS is web-native and translates 1:1 to the Dioxus webview).
const TOKENS_CSS: &str = include_str!("../assets/tokens.css");
const APP_CSS: &str = include_str!("../assets/app.css");
const DETAIL_CSS: &str = include_str!("../assets/detail.css");
const OVERLAYS_CSS: &str = include_str!("../assets/overlays.css");
const SCREENS_CSS: &str = include_str!("../assets/screens.css");
const CONTAINERS_CSS: &str = include_str!("../assets/containers.css");
const NAV_CSS: &str = include_str!("../assets/nav.css");
const XTERM_CSS: &str = include_str!("../assets/xterm/xterm.css");
const XTERM_JS: &str = include_str!("../assets/xterm/xterm.js");
const XTERM_FIT_JS: &str = include_str!("../assets/xterm/addon-fit.js");
/// Line-icon sprite referenced by `<use href="#i-…">`.
const SPRITE: &str = include_str!("../assets/sprite.svg");

/// App-feel + popover-menu styles (native-app cursor behavior, context menu,
/// namespace dropdown) — same token language as the ported design CSS.
const EXTRA_CSS: &str = r#"
/* Dedicated per-cluster accent palette: 6 widely-separated, status-safe hues
   (hue + lightness varied so adjacent clusters never look alike). */
[data-theme="dark"] {
  --kc-1: oklch(0.70 0.18 277);  /* indigo  */
  --kc-2: oklch(0.81 0.13 205);  /* sky     */
  --kc-3: oklch(0.77 0.16 52);   /* orange  */
  --kc-4: oklch(0.72 0.20 350);  /* pink    */
  --kc-5: oklch(0.84 0.12 178);  /* teal    */
  --kc-6: oklch(0.65 0.19 300);  /* violet  */
}
[data-theme="light"] {
  --kc-1: oklch(0.54 0.19 277);
  --kc-2: oklch(0.55 0.13 215);
  --kc-3: oklch(0.58 0.16 52);
  --kc-4: oklch(0.55 0.20 350);
  --kc-5: oklch(0.52 0.11 178);
  --kc-6: oklch(0.49 0.19 300);
}

/* Native-app feel: no text caret / text selection on chrome. */
.app { cursor: default; -webkit-user-select: none; user-select: none; }
.app input { cursor: text; -webkit-user-select: text; user-select: text; }

/* Popover menu — context menu + namespace dropdown */
.menu-scrim { position: fixed; inset: 0; z-index: var(--z-overlay); }
.menu {
  position: fixed; z-index: var(--z-palette);
  min-width: 184px; padding: var(--sp-2);
  background: var(--bg-overlay); border: 1px solid var(--border-default);
  border-radius: var(--radius-lg); box-shadow: var(--shadow-lg);
}
.menu.under { position: absolute; top: calc(100% + 6px); left: 0; }
.menu-head {
  font-size: var(--text-micro); color: var(--fg-faint);
  text-transform: uppercase; letter-spacing: var(--tracking-eyebrow);
  padding: var(--sp-3) var(--sp-4) var(--sp-2);
}
.menu-item {
  display: flex; align-items: center; gap: var(--sp-5);
  height: 30px; padding: 0 var(--sp-4); border-radius: var(--radius-sm);
  color: var(--fg-default); font-size: var(--text-small); cursor: pointer;
  white-space: nowrap; transition: background var(--dur-fast), color var(--dur-fast);
}
.menu-item:hover { background: var(--bg-raised); color: var(--fg-strong); }
.menu-item.hl { background: var(--accent-tint); color: var(--fg-strong); }
.menu-item.hl:hover { background: var(--accent-tint-strong); }
.menu-item svg { width: 15px; height: 15px; color: var(--fg-muted); flex: none; }
.menu-item.danger { color: var(--status-failed); }
.menu-item.danger:hover { background: var(--status-failed-tint); color: var(--status-failed); }
.menu-item.is-set { color: var(--accent-fg); }
.menu-item.is-set svg { color: var(--accent-fg); fill: color-mix(in oklch, var(--accent-fg) 85%, transparent); }
.menu-item .check { margin-left: auto; display: inline-flex; }
.menu-item .check svg { width: 14px; height: 14px; color: var(--accent-fg); }
.menu-sep { height: 1px; background: var(--border-subtle); margin: var(--sp-2) 0; }
/* Cluster switcher rows: name fills, pin reveals on hover (filled when pinned). */
.menu-item .ctx-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cl-pin {
  flex: none; width: 22px; height: 22px; margin-left: var(--sp-2); padding: 0; border: none;
  display: inline-flex; align-items: center; justify-content: center; border-radius: var(--radius-sm);
  background: transparent; color: var(--fg-faint); cursor: pointer; opacity: 0;
  transition: opacity var(--dur-fast), background var(--dur-fast), color var(--dur-fast);
}
.cl-pin svg { width: 13px; height: 13px; fill: none; stroke: currentColor; stroke-width: 1.6; }
.menu-item:hover .cl-pin { opacity: 1; }
.cl-pin:hover { background: var(--accent-tint); color: var(--accent-fg); }
.cl-pin.pinned { opacity: 1; color: var(--accent-fg); }
.menu-search {
  display: flex; align-items: center; gap: var(--sp-4);
  height: var(--control-h); margin: var(--sp-1) var(--sp-1) var(--sp-2);
  padding: 0 var(--sp-4); border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle); background: var(--bg-base);
}
.menu-search:focus-within { border-color: var(--accent); box-shadow: var(--shadow-focus); }
.menu-search svg { width: 15px; height: 15px; color: var(--fg-faint); flex: none; }
.menu-search input { all: unset; flex: 1; font-size: var(--text-small); color: var(--fg-default); min-width: 0; }
.menu-search input::placeholder { color: var(--fg-faint); }
.menu-scroll { max-height: 280px; overflow-y: auto; }
.menu-empty { padding: var(--sp-4); font-size: var(--text-small); color: var(--fg-faint); text-align: center; }
.ns-wrap { position: relative; display: inline-flex; align-items: center; }
/* Namespace view tabs */
.ns-tabs { display: flex; align-items: center; gap: var(--sp-3); }
/* "ns" label sitting to the left of the view tabs */
.ns-eyebrow {
  font-size: var(--text-micro); font-weight: var(--fw-semibold);
  text-transform: uppercase; letter-spacing: var(--tracking-eyebrow);
  color: var(--fg-faint); flex: none;
}
/* No reserved space at rest: the button grows on hover to accommodate the ×.
   The negative margin cancels the flex gap so the tab is fully compact idle. */
.ns-close {
  display: inline-flex; align-items: center; justify-content: center;
  width: 0; height: 16px; flex: none; overflow: hidden; border-radius: var(--radius-sm);
  margin-left: calc(-1 * var(--sp-3));
  color: var(--fg-faint); cursor: pointer; opacity: 0;
  transition: width var(--dur-fast) var(--ease-standard),
              margin-left var(--dur-fast) var(--ease-standard),
              opacity var(--dur-fast), background var(--dur-fast);
}
.ns-select.removable:hover .ns-close { width: 16px; margin-left: 0; opacity: 1; }
.ns-close:hover { background: var(--bg-raised); color: var(--fg-default); }
.ns-close svg { width: 11px; height: 11px; }
.ns-select.active { border-color: var(--accent); color: var(--fg-strong); }
.ns-tab { background: transparent; }
.ns-tab .val { color: var(--fg-muted); font-weight: var(--fw-regular); }
.ns-tab:hover .val { color: var(--fg-default); }
.ns-add {
  display: inline-flex; align-items: center; justify-content: center;
  width: var(--control-h); height: var(--control-h); flex: none;
  border-radius: var(--radius-md); border: 1px solid var(--border-subtle);
  background: var(--bg-base); color: var(--fg-muted); cursor: pointer;
  transition: border-color var(--dur-fast), color var(--dur-fast);
}
.ns-add:hover { border-color: var(--border-default); color: var(--fg-default); }
.ns-add svg { width: 15px; height: 15px; }
.cluster-wrap { position: relative; }
.ctx-accent { width: 3px; height: 16px; border-radius: 2px; flex: none; margin-right: 2px; }

/* ============================================================
   Unified title band (hidden/transparent titlebar layout)
   - Sidebar owns the full left edge top-to-bottom.
   - OS traffic lights sit in the sidebar's reserved top 48px.
   - Brand drops into the sidebar header, below the lights.
   - Top bar spans only the content column, leads with cluster.
   - The cluster hairline underlines the whole 48px band, doing
     double duty as band divider + "which cluster" signal.
   ============================================================ */
.app {
  grid-template-areas:
    "lights topbar"
    "nav    main";
}

/* Reserved traffic-light space over the sidebar; part of the drag band. */
.titlebar-pad {
  grid-area: lights;
  background: var(--bg-base);
  border-right: 1px solid var(--border-subtle);
  -webkit-app-region: drag; app-region: drag;
}

/* Top bar now spans only the content column and is a drag region. */
.topbar {
  grid-area: topbar;
  padding-left: var(--sp-5);
  -webkit-app-region: drag; app-region: drag;
}

/* Sidebar starts under the band and carries the brand header. */
.nav { grid-area: nav; padding-top: var(--sp-4); }
.sidebar-brand {
  display: flex; align-items: center; gap: var(--sp-4);
  padding: var(--sp-2) var(--sp-4) var(--sp-5);
}

/* The hairline underlines the entire band, across both columns. */
.cluster-hairline { top: 48px; }

/* Interactive controls must never drag the window. */
.topbar button,
.topbar input,
.topbar label,
.topbar .ns-wrap,
.menu { -webkit-app-region: no-drag; app-region: no-drag; }

/* Windows / Linux: no OS lights — collapse the reserved cell and show
   window controls at the top-bar's right edge instead. */
[data-platform="other"] .titlebar-pad { display: none; }
.win-controls { display: none; align-items: center; gap: var(--sp-2); -webkit-app-region: no-drag; app-region: no-drag; }
[data-platform="other"] .win-controls { display: inline-flex; }

/* Detail panel needs a positioned ancestor + a drag-capture overlay. */
.main { position: relative; }
.resize-capture { position: fixed; inset: 0; z-index: var(--z-panel); cursor: col-resize; }
.detail-headctrls .icon-btn { width: 28px; height: 28px; }
.detail-empty, .detail-loading {
  padding: var(--sp-9) var(--sp-7); color: var(--fg-muted); font-size: var(--text-small);
}
.code .ln code .raw { color: var(--fg-default); }

/* Detail action toolbar */
.detail-actions { padding: var(--sp-6) var(--sp-7) var(--sp-5); }
.scale-group { display: inline-flex; align-items: center; }
.scale-label {
  height: 28px; display: inline-flex; align-items: center; padding: 0 var(--sp-4);
  font-size: var(--text-small); color: var(--fg-muted); white-space: nowrap;
  background: var(--bg-raised); border: 1px solid var(--border-subtle); border-right: none;
  border-radius: var(--radius-md) 0 0 var(--radius-md);
}
.scale-input {
  all: unset; width: 44px; height: 28px; padding: 0 var(--sp-3);
  border-top: 1px solid var(--border-subtle); border-bottom: 1px solid var(--border-subtle);
  border-left: 1px solid var(--border-subtle);
  background: var(--bg-base); color: var(--fg-default);
  font-size: var(--text-small); font-variant-numeric: tabular-nums; text-align: center;
}
.scale-input:focus { border-color: var(--accent); box-shadow: var(--shadow-focus); position: relative; z-index: 1; }
.scale-go {
  height: 28px; width: 32px; flex: none; display: inline-flex; align-items: center; justify-content: center;
  border: 1px solid var(--border-subtle); border-radius: 0 var(--radius-md) var(--radius-md) 0;
  background: var(--bg-raised); color: var(--fg-muted); cursor: pointer;
  transition: background var(--dur-fast), color var(--dur-fast), border-color var(--dur-fast);
}
.scale-go svg { width: 15px; height: 15px; }
.scale-go:not(:disabled):hover { background: var(--accent-tint); color: var(--accent-fg); border-color: var(--accent); }
.scale-go:disabled { opacity: 0.4; cursor: default; }
.btn:disabled { opacity: 0.45; cursor: default; pointer-events: none; }

/* Editable YAML textarea */
/* Overlay editor: a transparent textarea (real caret/editing) layered over a
   syntax + search highlighted backdrop. Both share identical text metrics so
   the glyphs register exactly; the textarea drives scroll, the backdrop mirrors it. */
.yaml-editor { position: relative; flex: 1; min-height: 0; overflow: hidden; background: var(--bg-inset); }
.yaml-editor .yaml-hl,
.yaml-editor .yaml-edit {
  position: absolute; inset: 0; margin: 0; border: 0; overflow: auto;
  font-family: var(--font-mono); font-size: var(--text-small); line-height: 1.7;
  padding: var(--sp-5) var(--sp-6); white-space: pre; tab-size: 2;
  letter-spacing: normal; word-spacing: normal;
}
.yaml-editor .yaml-hl { z-index: 0; color: var(--fg-default); pointer-events: none; }
.yaml-edit {
  all: unset; box-sizing: border-box;
  position: absolute; inset: 0; z-index: 1; overflow: auto;
  font-family: var(--font-mono); font-size: var(--text-small); line-height: 1.7;
  padding: var(--sp-5) var(--sp-6); white-space: pre; tab-size: 2;
  background: transparent; color: transparent; caret-color: var(--fg-strong);
  -webkit-user-select: text; user-select: text; cursor: text;
}
.yaml-edit::selection { background: var(--accent-tint); color: transparent; }

/* Toasts */
.toasts {
  position: fixed; right: var(--sp-7); bottom: var(--sp-7);
  display: flex; flex-direction: column; gap: var(--sp-4); z-index: var(--z-toast);
}
/* Update-available banner — pill below the topbar. */
.update-banner {
  position: fixed; left: 50%; top: 58px; transform: translateX(-50%);
  z-index: var(--z-toast); display: flex; align-items: center; gap: var(--sp-4);
  padding: var(--sp-3) var(--sp-3) var(--sp-3) var(--sp-5);
  background: var(--bg-overlay); border: 1px solid var(--accent);
  border-radius: var(--radius-full); box-shadow: var(--shadow-lg);
  font-size: var(--text-small); color: var(--fg-default);
  animation: toast-in var(--dur-base) var(--ease-out);
}
.update-banner .ub-mark { width: 16px; height: 16px; display: inline-flex; }
.update-banner .ub-text b { color: var(--fg-strong); font-weight: var(--fw-semibold); }
.update-banner .ub-btn {
  display: inline-flex; align-items: center; gap: var(--sp-3);
  height: 26px; padding: 0 var(--sp-4); border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle); background: var(--bg-raised);
  color: var(--fg-default); font-size: var(--text-small); cursor: pointer;
  font-variant-numeric: tabular-nums;
}
.update-banner .ub-btn svg { width: 14px; height: 14px; }
.update-banner .ub-btn:hover { border-color: var(--accent); color: var(--accent-fg); }
.update-banner .ub-btn.ghost { background: transparent; border-color: transparent; color: var(--fg-muted); }
.update-banner .ub-btn.ghost:hover { color: var(--fg-default); }
.update-banner .ub-x {
  width: 24px; height: 24px; border: none; background: transparent; cursor: pointer;
  color: var(--fg-faint); display: inline-flex; align-items: center; justify-content: center;
  border-radius: var(--radius-sm);
}
.update-banner .ub-x:hover { background: var(--bg-raised); color: var(--fg-default); }
.update-banner .ub-x svg { width: 14px; height: 14px; }
.toast {
  display: flex; align-items: center; gap: var(--sp-4);
  min-width: 280px; max-width: 400px; padding: var(--sp-4) var(--sp-4) var(--sp-4) var(--sp-5);
  background: var(--bg-overlay); border: 1px solid var(--border-default);
  border-radius: var(--radius-lg); box-shadow: var(--shadow-lg);
  font-size: var(--text-small); color: var(--fg-default);
  animation: toast-in var(--dur-base) var(--ease-out);
}
@keyframes toast-in { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
.toast .ic { width: 18px; height: 18px; flex: none; }
.toast.ok .ic { color: var(--status-running); }
.toast.err .ic { color: var(--status-failed); }
.toast .msg { flex: 1; word-break: break-word; }

/* Centered-dialog entrance: keep translateX(-50%) through the animation so the
   dialog doesn't start off-center and snap to the middle when it ends. */
@keyframes dialog-in {
  from { opacity: 0; transform: translateX(-50%) translateY(8px); }
  to   { opacity: 1; transform: translateX(-50%) translateY(0); }
}

/* Delete confirmation dialog */
.confirm-dialog {
  position: fixed; left: 50%; top: 30%; transform: translateX(-50%);
  z-index: var(--z-palette); width: min(400px, 90vw);
  display: flex; flex-direction: column; align-items: center; gap: var(--sp-5);
  padding: var(--sp-8) var(--sp-7); text-align: center;
  background: var(--bg-overlay); border: 1px solid var(--border-default);
  border-radius: var(--radius-xl); box-shadow: var(--shadow-lg);
  animation: dialog-in var(--dur-base) var(--ease-out);
}
.cd-icon {
  display: inline-flex; align-items: center; justify-content: center;
  width: 40px; height: 40px; border-radius: var(--radius-full);
  background: var(--status-failed-tint); color: var(--status-failed);
}
.cd-icon svg { width: 20px; height: 20px; }
.cd-title { font-size: var(--text-heading); font-weight: var(--fw-semibold); color: var(--fg-strong); text-wrap: balance; }
.cd-body { font-size: var(--text-small); color: var(--fg-muted); }
.cd-actions { display: flex; gap: var(--sp-4); margin-top: var(--sp-3); }
.cd-actions .btn { height: var(--control-h); padding: 0 var(--sp-6); }
.btn-danger-solid { background: var(--status-failed); border-color: transparent; color: #fff; }
.btn-danger-solid:hover { background: var(--status-failed); color: #fff; filter: brightness(1.08); }

/* About dialog */
.about-dialog {
  position: fixed; left: 50%; top: 30%; transform: translateX(-50%);
  z-index: var(--z-palette); width: min(340px, 90vw);
  display: flex; flex-direction: column; align-items: center; gap: var(--sp-4);
  padding: var(--sp-9) var(--sp-7) var(--sp-8); text-align: center;
  background: var(--bg-overlay); border: 1px solid var(--border-default);
  border-radius: var(--radius-xl); box-shadow: var(--shadow-lg);
  animation: dialog-in var(--dur-base) var(--ease-out);
}
.about-mark { width: 46px; height: 46px; color: var(--accent); margin-bottom: var(--sp-2); }
.about-name { font-size: var(--text-title); font-weight: var(--fw-semibold); color: var(--fg-strong); letter-spacing: var(--tracking-tight); }
.about-ver { font-size: var(--text-small); color: var(--fg-faint); font-variant-numeric: tabular-nums; margin-top: calc(-1 * var(--sp-2)); }
.about-upd { font-size: var(--text-small); color: var(--fg-muted); min-height: 16px; }
.about-upd-link { border: none; background: transparent; cursor: pointer; color: var(--accent-fg); font-size: var(--text-small); padding: 0; }
.about-upd-link:hover { text-decoration: underline; }
.about-love { font-size: var(--text-body); color: var(--fg-muted); margin-top: var(--sp-2); }
.about-love .heart { color: var(--status-failed); }
.about-love b { color: var(--fg-default); font-weight: var(--fw-medium); }
.about-repo {
  display: inline-flex; align-items: center; gap: var(--sp-4); margin-top: var(--sp-4);
  font-size: var(--text-small); color: var(--accent-fg); font-family: var(--font-mono);
  background: var(--bg-raised); border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md); padding: var(--sp-3) var(--sp-5);
  cursor: pointer; transition: border-color var(--dur-fast), background var(--dur-fast), color var(--dur-fast);
}
.about-repo:hover { border-color: var(--accent); background: var(--accent-tint); color: var(--accent-fg); }
.about-repo svg { width: 14px; height: 14px; }

/* Kind tabs (Pods | Deployments | …) */
.kind-tabs { display: flex; gap: var(--sp-2); }
.kind-tab {
  background: none; border: none; cursor: pointer;
  font-family: var(--font-sans); font-size: var(--text-small); font-weight: var(--fw-medium);
  color: var(--fg-muted); padding: var(--sp-3) var(--sp-4); border-radius: var(--radius-md);
  transition: background var(--dur-fast), color var(--dur-fast);
}
.kind-tab:hover { color: var(--fg-default); background: var(--bg-raised); }
.kind-tab.active { color: var(--fg-strong); background: var(--accent-tint); }

/* Center the row checkbox vertically (inline-flex otherwise sits on baseline). */
.table tbody td.col-check { vertical-align: middle; }
.row-check { vertical-align: middle; }

/* Stable column widths under virtualization (Name absorbs the slack). */
.table { table-layout: fixed; }
.table td, .table th { overflow: hidden; }
.table .cell-ns, .table .cell-age, .cell-name .nm, .cell-name .meta { text-overflow: ellipsis; }

/* Keep nav items their natural height (don't compress when many) — scroll instead. */
.nav { overflow-y: auto; }
.nav-item { flex: none; }

/* Disabled nav category (no kinds discovered). */
.nav-item.disabled { opacity: 0.38; cursor: default; }
.nav-item.disabled:hover { background: transparent; color: var(--fg-muted); }

/* Expandable nav (groups, guide rail, default-pin) lives in nav.css. */

/* Data tab (ConfigMap / Secret key-values) */
.data-row { padding: var(--sp-5) 0; border-bottom: 1px solid var(--border-subtle); display: flex; flex-direction: column; gap: var(--sp-3); }
.data-row:last-child { border-bottom: none; }
.data-head { display: flex; align-items: center; gap: var(--sp-4); }
.data-key { font-family: var(--font-mono); font-size: var(--text-small); color: var(--fg-strong); font-weight: var(--fw-medium); word-break: break-all; }
.data-btn {
  display: inline-flex; align-items: center; justify-content: center;
  width: 26px; height: 26px; border-radius: var(--radius-md);
  border: 1px solid transparent; background: transparent; color: var(--fg-muted); cursor: pointer;
  transition: background var(--dur-fast), color var(--dur-fast);
}
.data-btn:hover { background: var(--bg-raised); color: var(--fg-strong); }
.data-btn svg { width: 15px; height: 15px; }
.data-copy { margin-left: auto; }
/* Copy button: swap copy icon → check on success, with a spring pop. */
.copy-btn .copy-ico { display: inline-flex; align-items: center; justify-content: center; }
.copy-btn.copied { color: var(--status-running); }
.copy-btn.copied .copy-ico svg { animation: copy-pop 240ms cubic-bezier(.34, 1.56, .64, 1); }
@keyframes copy-pop {
  0%   { transform: scale(.3); opacity: 0; }
  55%  { transform: scale(1.18); opacity: 1; }
  100% { transform: scale(1); }
}
.data-val {
  font-family: var(--font-mono); font-size: var(--text-small); color: var(--fg-default);
  background: var(--bg-inset); border-radius: var(--radius-md); padding: var(--sp-4);
  margin: 0; white-space: pre-wrap; word-break: break-all; max-height: 260px; overflow: auto;
  -webkit-user-select: text; user-select: text; cursor: text;
}
.data-val.masked { color: var(--fg-faint); letter-spacing: 2px; }

/* Topbar tooltips must drop BELOW their control — above would clip past the
   top of the window. */
.topbar .tip::after {
  top: calc(100% + 6px); bottom: auto;
  transform: translateX(-50%) translateY(-4px);
}
.topbar .tip:hover::after { transform: translateX(-50%) translateY(0); }
/* Right-side topbar tooltips anchor to the right edge so they don't clip off-window. */
.topbar-right .tip::after { left: auto; right: 0; transform: translateX(0) translateY(-4px); }
.topbar-right .tip:hover::after { transform: translateX(0) translateY(0); }
/* The freshness/stale indicator can carry a long connection error — let it wrap
   into a readable box instead of one clipped off-window line. */
.freshness.tip::after {
  white-space: pre-wrap; width: max-content; max-width: 380px;
  max-height: 50vh; overflow: hidden; text-align: left;
  line-height: var(--lh-snug); word-break: break-word;
}

/* Detail-panel header tooltips: drop below + right-anchored so they don't clip
   against the band or the panel's right edge. */
.detail-headctrls .tip::after {
  top: calc(100% + 6px); bottom: auto;
  left: auto; right: 0;
  transform: translateY(-4px);
}
.detail-headctrls .tip:hover::after { transform: translateY(0); }

/* Action-toolbar tooltips drop below too. */
.detail-actions .tip::after {
  top: calc(100% + 6px); bottom: auto;
  transform: translateX(-50%) translateY(-4px);
}
.detail-actions .tip:hover::after { transform: translateX(-50%) translateY(0); }

/* Exclude (anti-filter) input — red accent signals "hide matching". */
.logs-toolbar .search.exclude { max-width: 200px; }
.search.exclude svg { color: var(--status-failed); }
.search.exclude:focus-within {
  border-color: var(--status-failed);
  box-shadow: 0 0 0 3px color-mix(in oklch, var(--status-failed) 28%, transparent);
}

/* Port-forward rows in the pod Summary */
.pf-row { display: flex; align-items: center; gap: var(--sp-5); padding: var(--sp-3) 0; border-bottom: 1px solid var(--border-subtle); }
.pf-row:last-child { border-bottom: none; }
.pf-row .btn { margin-left: auto; height: 26px; }
.pf-fwd { display: inline-flex; align-items: center; gap: var(--sp-3); font-family: var(--font-mono); font-size: var(--text-small); color: var(--fg-strong); }
.pf-fwd .d { width: 7px; height: 7px; border-radius: 50%; background: var(--status-running); }
.pf-port { font-family: var(--font-mono); font-size: var(--text-small); color: var(--fg-muted); }

/* Spinner for loading states */
.kspin {
  width: 22px; height: 22px; border-radius: 50%;
  border: 2.5px solid var(--border-default); border-top-color: var(--accent);
  animation: kspin 0.8s linear infinite;
}
@keyframes kspin { to { transform: rotate(360deg); } }
.kload { display: flex; flex-direction: column; align-items: center; gap: var(--sp-5); padding: var(--sp-12); color: var(--fg-muted); }

/* Metric sparkline cells */
.metric-cell { display: flex; align-items: center; gap: var(--sp-4); }
.metric-val { font-variant-numeric: tabular-nums; color: var(--fg-muted); font-size: var(--text-micro); white-space: nowrap; }

/* Exec terminal (xterm.js host). */
.exec-wrap { height: 100%; background: var(--bg-inset); padding: var(--sp-4) var(--sp-5); }
.exec-term { height: 100%; width: 100%; }

/* Logs + YAML are content the user copies — allow selection + text caret. */
.logview, .logview *, .code, .code * {
  -webkit-user-select: text; user-select: text; cursor: text;
}
"#;

/// Fixed table row height (comfortable density, matches `--row-h`). Drives
/// virtualization math.
const ROW_H: f64 = 44.0;
/// Extra rows rendered above/below the viewport to avoid blank flashes on scroll.
const OVERSCAN: usize = 8;

/// Ordered left-nav categories (icon, label). Kinds are bucketed into these at
/// runtime from the discovered catalog via `KindMeta::category`.
const CATS: &[(&str, &str)] = &[
    ("i-workloads", "Workloads"),
    ("i-network", "Network"),
    ("i-config", "Config"),
    ("i-storage", "Storage"),
    ("i-nodes", "Nodes"),
    ("i-events", "Events"),
    ("i-cluster", "Cluster"),
    ("i-related", "Custom Resources"),
    ("i-summary", "Other"),
];

/// Command sender (UI → engine). Set once in `main`.
static CMD: OnceLock<tokio::sync::mpsc::UnboundedSender<Cmd>> = OnceLock::new();

fn send_cmd(cmd: Cmd) {
    if let Some(tx) = CMD.get() {
        let _ = tx.send(cmd);
    }
}

/// Hand-off slot for the delta receiver: created in `main`, taken once by the
/// UI coroutine. Phase 2 replaces this with a `CoreHandle` via Dioxus context.
static RX: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Delta>>> = Mutex::new(None);

/// Ask the user's login+interactive shell for its `$PATH`, bounded by `timeout`
/// so a slow/hanging shell rc can't stall startup. Returns None on timeout/error.
#[cfg(target_os = "macos")]
fn shell_path_via(shell: &str, timeout: std::time::Duration) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(shell)
        .args(["-ilc", "echo $PATH"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                // PATH is small (well under the pipe buffer), so reading after
                // exit can't deadlock.
                use std::io::Read;
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return Some(out);
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Err(_) => return None,
        }
    }
}

/// Human-friendly text for the freshness/"Stale" bar tooltip on a connection
/// error — with healing steps for the common "auth exec plugin not on PATH" case.
fn conn_error_tip(e: &str) -> String {
    let lower = e.to_lowercase();
    let auth_exec_missing = lower.contains("auth exec")
        && (lower.contains("no such file")
            || lower.contains("os error 2")
            || lower.contains("not found")
            || lower.contains("executable file not found"));
    // The plugin ran but exited non-zero — almost always expired/invalid creds.
    let auth_exec_failed = lower.contains("auth exec command") && lower.contains("failed with status");
    if auth_exec_missing {
        format!(
            "Can't reach the cluster: its kubeconfig runs an auth plugin, but that \
             program isn't on Kompass's PATH.\n\n\
             To fix:\n\
             •  Install the auth helper your kubeconfig uses — e.g. awscli (aws), \
             gke-gcloud-auth-plugin, or kubelogin.\n\
             •  Confirm it's on your shell PATH (in Terminal: which aws / gcloud / kubelogin).\n\
             •  Quit and reopen Kompass so it re-reads your PATH.\n\n\
             {e}"
        )
    } else if auth_exec_failed {
        // Tailor the refresh command to the provider (and AWS profile if present).
        let refresh = if lower.contains("eks get-token") || lower.contains("aws") {
            match e.split("AWS_PROFILE=\"").nth(1).and_then(|s| s.split('"').next()) {
                Some(p) if !p.is_empty() => {
                    format!("aws sso login --profile {p}  (or refresh that profile's credentials)")
                }
                _ => "aws sso login  (or refresh your AWS credentials)".to_string(),
            }
        } else if lower.contains("gke-gcloud") || lower.contains("gcloud") {
            "gcloud auth login".to_string()
        } else if lower.contains("kubelogin") || lower.contains("az ") {
            "az login".to_string()
        } else {
            "refresh your cluster credentials".to_string()
        };
        format!(
            "Reached the cluster's auth plugin, but it failed — usually expired or \
             missing credentials.\n\n\
             To fix:\n\
             •  In a terminal, refresh credentials: {refresh}\n\
             •  Then click Retry.\n\n\
             {e}"
        )
    } else {
        format!("Connection error: {e}")
    }
}

fn main() {
    // Inject common CLI paths for macOS app bundles (where PATH is just /usr/bin:/bin)
    // so Kubernetes auth-exec plugins (aws-iam-authenticator, gke-gcloud-auth-plugin) can be found.
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = std::env::var_os("PATH") {
            let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
            
            // 1. Try to inherit the exact PATH from the user's interactive shell
            //    (where they set up aws/gcloud/kubelogin). Bounded by a timeout so
            //    a slow/hanging shell rc can't block app startup.
            if let Ok(shell) = std::env::var("SHELL") {
                if let Some(shell_path) = shell_path_via(&shell, std::time::Duration::from_secs(2)) {
                    for p in std::env::split_paths(shell_path.trim()) {
                        if !paths.contains(&p) {
                            paths.push(p);
                        }
                    }
                }
            }

            // 2. Fallback / ensure common locations are present
            let mut extras = vec![
                std::path::PathBuf::from("/usr/local/bin"),
                std::path::PathBuf::from("/opt/homebrew/bin"),
            ];
            if let Ok(home) = std::env::var("HOME") {
                extras.push(std::path::PathBuf::from(format!("{home}/.local/bin")));
                extras.push(std::path::PathBuf::from(format!("{home}/.krew/bin")));
                extras.push(std::path::PathBuf::from(format!("{home}/.cargo/bin")));
                // Common google-cloud-sdk paths
                extras.push(std::path::PathBuf::from(format!("{home}/google-cloud-sdk/bin")));
                extras.push(std::path::PathBuf::from(format!("{home}/Downloads/google-cloud-sdk/bin")));
            }
            for extra in extras {
                if !paths.contains(&extra) {
                    paths.push(extra);
                }
            }
            if let Ok(new_path) = std::env::join_paths(paths) {
                std::env::set_var("PATH", new_path);
            }
        }
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let (tx, rx) = unbounded_channel::<Delta>();
    *RX.lock().unwrap() = Some(rx);

    let (cmd_tx, cmd_rx) = unbounded_channel::<Cmd>();
    CMD.set(cmd_tx).ok();

    // Engine runs on its own multi-threaded tokio runtime, off the UI thread.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(run_engine(tx, cmd_rx));
    });

    // macOS: transparent + fullsize-content titlebar so the dark app chrome
    // runs under the traffic lights (Linear/Raycast-style). Not always-on-top.
    let mut window = WindowBuilder::new()
        .with_title("Kompass")
        .with_always_on_top(false)
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true);

    if let Some(icon) = app_icon() {
        window = window.with_window_icon(Some(icon));
    }

    dioxus::LaunchBuilder::desktop()
        .with_cfg(Config::new().with_window(window))
        .launch(App);
}

/// An open right-click context menu, anchored at the cursor.
#[derive(Clone, PartialEq)]
struct CtxMenu {
    x: f64,
    y: f64,
    target: DetailTarget,
}

/// Right-click menu on a nav row — set/clear the default boot page.
#[derive(Clone, PartialEq)]
struct NavMenu {
    x: f64,
    y: f64,
    key: String,
    label: String,
    is_default: bool,
}

/// A navigable view for ⌘[ / ⌘]. The full "where you are": page (overview vs a
/// resource kind), which namespace-view tab, that tab's namespace, and the
/// search filter. `filter` is updated in place on the current entry (so typing
/// doesn't spam history) and restored on back/forward.
#[derive(Clone, PartialEq)]
struct ViewState {
    overview: bool,
    kind: String,
    ns_active: usize,
    namespace: Option<String>,
    filter: String,
}

impl ViewState {
    /// Identity for history dedup — everything except the live filter text.
    fn same_place(&self, o: &ViewState) -> bool {
        self.overview == o.overview
            && self.kind == o.kind
            && self.ns_active == o.ns_active
            && self.namespace == o.namespace
    }
}

/// A transient result notification.
#[derive(Clone, PartialEq)]
struct Toast {
    id: u64,
    ok: bool,
    msg: String,
}

/// A delete request, routed through confirmation (skipped for Pods).
#[derive(Clone)]
struct DeleteReq {
    is_pod: bool,
    message: String,
    cmds: Vec<Cmd>,
    close_detail: bool,
}

/// A ⌘K palette action.
#[derive(Clone, PartialEq)]
enum PalAction {
    Kind(String),
    Context(String),
    Open(DetailTarget),
    ToggleTheme,
}

/// A ⌘K palette result row.
#[derive(Clone, PartialEq)]
struct PalItem {
    group: &'static str,
    icon: &'static str,
    title: String,
    sub: String,
    action: PalAction,
}

/// Per-kind column visibility.
#[derive(Clone, PartialEq)]
struct ColVis {
    ns: bool,
    status: bool,
    age: bool,
    cpu: bool,
    mem: bool,
    cols: Vec<bool>,
}

/// Sortable table column.
#[derive(Clone, Copy, PartialEq)]
enum SortKey {
    Name,
    Namespace,
    Status,
    Age,
    Col(usize),
    Cpu,
    Mem,
}

impl SortKey {
    fn id(self) -> String {
        match self {
            SortKey::Name => "name".into(),
            SortKey::Namespace => "namespace".into(),
            SortKey::Status => "status".into(),
            SortKey::Age => "age".into(),
            SortKey::Col(i) => format!("col:{i}"),
            SortKey::Cpu => "cpu".into(),
            SortKey::Mem => "mem".into(),
        }
    }
    fn from_id(s: &str) -> SortKey {
        match s {
            "namespace" => SortKey::Namespace,
            "status" => SortKey::Status,
            "age" => SortKey::Age,
            "cpu" => SortKey::Cpu,
            "mem" => SortKey::Mem,
            _ if s.starts_with("col:") => {
                SortKey::Col(s[4..].parse().unwrap_or(0))
            }
            _ => SortKey::Name,
        }
    }
}

/// Parse a humanized age ("5m", "3h", "2d") back to seconds for sorting.
fn parse_age_secs(age: &str) -> i64 {
    let age = age.trim();
    let (num, mult) = match age.chars().last() {
        Some('s') => (&age[..age.len() - 1], 1),
        Some('m') => (&age[..age.len() - 1], 60),
        Some('h') => (&age[..age.len() - 1], 3600),
        Some('d') => (&age[..age.len() - 1], 86_400),
        _ => (age, 1),
    };
    num.parse::<i64>().unwrap_or(0) * mult
}

/// Compare two column cells: numeric if both lead with a number, else string.
fn col_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let na = a.split(['/', ' ']).next().and_then(|s| s.parse::<f64>().ok());
    let nb = b.split(['/', ' ']).next().and_then(|s| s.parse::<f64>().ok());
    match (na, nb) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}

/// Which detail-panel tab is showing.
#[derive(Clone, Copy, PartialEq)]
enum DetailTab {
    Summary,
    Data,
    Yaml,
    Logs,
    Exec,
    Events,
}

/// The resource a detail panel is showing (header data, available instantly
/// from the table row while the manifest loads).
#[derive(Clone, PartialEq)]
struct DetailTarget {
    kind_id: String,
    kind_name: String,
    namespace: String,
    name: String,
    status: String,
    status_class: String,
    cols: Vec<String>,
    age: String,
}

/// A single streamed log line (with its source pod + color index).
#[derive(Clone, PartialEq)]
struct LogEntry {
    source: String,
    idx: u8,
    line: String,
}

/// Request to open the detail panel on a target + tab.
#[derive(Clone, PartialEq)]
struct OpenReq {
    target: DetailTarget,
    tab: DetailTab,
}

/// The Rose compass app icon (indigo squircle). Used for the window icon;
/// the macOS dock icon comes from the bundled `Kompass.icns` once packaged.
fn app_icon() -> Option<Icon> {
    const ICON_PNG: &[u8] = include_bytes!("../assets/icon/icon_256.png");
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// Build SVG polyline points (in a 56×18 box) for a sparkline series.
fn sparkline_points(series: &[i64]) -> String {
    if series.len() < 2 {
        return String::new();
    }
    let max = series.iter().copied().max().unwrap_or(1).max(1) as f64;
    let n = series.len() as f64;
    series
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = i as f64 / (n - 1.0) * 56.0;
            let y = 18.0 - (v as f64 / max * 16.0) - 1.0;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_cpu(milli: i64) -> String {
    if milli >= 1000 {
        format!("{:.2} cores", milli as f64 / 1000.0)
    } else {
        format!("{milli}m")
    }
}

fn fmt_mem(bytes: i64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.1} Gi", mib / 1024.0)
    } else {
        format!("{:.0} Mi", mib)
    }
}

/// Map a container state to (square class, status token var, display label).
fn square_state(c: &ContainerState) -> (&'static str, &'static str, String) {
    if c.state == "Running" {
        if c.ready {
            ("running", "--status-running", "Running".into())
        } else {
            ("notready", "--status-running", "Running · not ready".into())
        }
    } else {
        match c.class.as_str() {
            "failed" => ("failed", "--status-failed", c.state.clone()),
            "pending" => ("pending", "--status-pending", c.state.clone()),
            _ => ("neutral", "--status-neutral", c.state.clone()),
        }
    }
}

/// Per-container status squares (Lens-style) with hover tooltips + `+N` rollup.
fn csq_square(c: &ContainerState, size: &str, first: bool) -> Element {
    let (cls, tok, label) = square_state(c);
    let rs = if c.restarts == 1 { "1 restart".to_string() } else { format!("{} restarts", c.restarts) };
    let wrap = if first { "csq-wrap tip-start" } else { "csq-wrap" };
    let is_init = c.kind == "init";
    rsx! {
        span { class: "{wrap}", key: "{c.kind}/{c.name}",
            span { class: "csq csq--{cls} {size}", tabindex: "0" }
            span { class: "cqtip",
                span { class: "cq-name", "{c.name}" if is_init { span { class: "cq-kind", "init" } } }
                span { class: "cq-state", i { style: "background:var({tok})" } "{label}" }
                span { class: "cq-rs", "{rs}" }
                i { class: "cq-caret" }
            }
        }
    }
}

#[component]
fn ContainerSquares(containers: Vec<ContainerState>, large: bool) -> Element {
    let size = if large { "csq-lg" } else { "csq-sm" };
    let cap = 14usize;
    let init: Vec<ContainerState> = containers.iter().filter(|c| c.kind == "init").cloned().collect();
    let main: Vec<ContainerState> = containers.iter().filter(|c| c.kind != "init").cloned().collect();

    // Cap the main group only (init always shown in full).
    let (shown, hidden): (&[ContainerState], &[ContainerState]) = if main.len() > cap {
        (&main[..cap - 1], &main[cap - 1..])
    } else {
        (&main[..], &[])
    };
    let worst = hidden
        .iter()
        .map(|c| match square_state(c).0 {
            "failed" => 4,
            "pending" => 3,
            "notready" => 2,
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let worst_cls = match worst {
        4 => "csq-more worst-failed",
        3 => "csq-more worst-pending",
        2 => "csq-more worst-notready",
        _ => "csq-more",
    };
    let hidden_n = hidden.len();

    rsx! {
        span { class: "csqs",
            if !init.is_empty() {
                span { class: "csq-grp",
                    for (i, c) in init.iter().enumerate() {
                        {csq_square(c, size, i == 0)}
                    }
                }
                span { class: "csq-sep" }
            }
            span { class: "csq-grp",
                for (i, c) in shown.iter().enumerate() {
                    {csq_square(c, size, init.is_empty() && i == 0)}
                }
                if hidden_n > 0 {
                    span { class: "{worst_cls} tip", "data-tip": "{hidden_n} more containers", "+{hidden_n}" }
                }
            }
        }
    }
}

/// Render an icon from the sprite.
/// Open a URL in the user's default browser.
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
}

/// Title-cased label from a kind id ("deployments.apps" → "Deployments"). Used
/// as a fallback before the discovered catalog (with real labels) loads.
fn label_from_id(id: &str) -> String {
    let plural = id.split('.').next().unwrap_or(id);
    let mut chars = plural.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => id.to_string(),
    }
}

/// Dotted-version compare: true if `latest` > `current` (tolerates a leading 'v',
/// ignores any pre-release suffix). e.g. is_newer("v1.2.0", "1.10.0") == false.
fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(s: &str) -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|seg| {
                seg.split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    }
    let (l, c) = (parts(latest), parts(current));
    for i in 0..l.len().max(c.len()) {
        let (a, b) = (l.get(i).copied().unwrap_or(0), c.get(i).copied().unwrap_or(0));
        if a != b {
            return a > b;
        }
    }
    false
}

/// Latest release tag from the GitHub API (via curl, always present on macOS).
fn latest_release_tag() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/erango/kompass/releases/latest",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v["tag_name"].as_str().map(String::from)
}

/// Best-effort: was this app installed via the Homebrew cask?
fn installed_via_brew() -> bool {
    std::process::Command::new("brew")
        .args(["list", "--cask", "kompass"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn icon(id: &str, stroke_width: &str) -> Element {
    let inner = format!("<use href=\"#{id}\"/>");
    rsx! {
        svg {
            "viewBox": "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "{stroke_width}",
            dangerous_inner_html: "{inner}",
        }
    }
}

/// A row in the cluster switcher: accent dot + name + pin toggle (+ active check).
/// Pinned rows show a filled pin; others reveal a ghost pin on hover.
#[component]
fn ClusterRow(
    name: String,
    active: bool,
    pinned: bool,
    on_pick: EventHandler<String>,
    on_pin: EventHandler<String>,
) -> Element {
    let accent = format!("var(--kc-{})", cluster_accent_index(&name));
    let (n_pick, n_pin) = (name.clone(), name.clone());
    rsx! {
        div {
            class: "menu-item",
            onclick: move |_| on_pick.call(n_pick.clone()),
            span { class: "ctx-accent", style: "background: {accent}" }
            span { class: "ctx-name", "{name}" }
            button {
                class: if pinned { "cl-pin pinned tip" } else { "cl-pin tip" },
                "data-tip": if pinned { "Unpin" } else { "Pin to top" },
                onclick: move |e| { e.stop_propagation(); on_pin.call(n_pin.clone()); },
                {icon("i-pin", "1.6")}
            }
            if active {
                span { class: "check", {icon("i-check", "3")} }
            }
        }
    }
}

/// A debounced search input. The text is held locally so typing is always
/// instant (only this component re-renders per keystroke); `on_change` fires
/// ~140ms after the last keystroke, so the heavy parent (list/highlight) only
/// rebuilds on a calm cadence. `value` syncs external resets (e.g. clear on nav).
#[component]
fn SearchBox(
    value: String,
    placeholder: String,
    #[props(default = "search".to_string())] class: String,
    #[props(default = "i-search".to_string())] icon_id: String,
    on_change: EventHandler<String>,
    #[props(default)] on_enter: Option<EventHandler<()>>,
) -> Element {
    let mut raw = use_signal(|| value.clone());
    // Re-sync the field when the external value changes out from under us.
    use_effect(use_reactive!(|value| {
        if *raw.peek() != value {
            raw.set(value);
        }
    }));
    let mut generation = use_signal(|| 0u64);
    rsx! {
        label { class: "{class}",
            {icon(&icon_id, "1.8")}
            input {
                r#type: "text",
                placeholder: "{placeholder}",
                autocomplete: "off",
                autocapitalize: "off",
                spellcheck: "false",
                "autocorrect": "off",
                value: "{raw}",
                oninput: move |e| {
                    let v = e.value();
                    raw.set(v.clone());
                    let g = *generation.peek() + 1;
                    generation.set(g);
                    spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
                        if *generation.peek() == g {
                            on_change.call(v);
                        }
                    });
                },
                onkeydown: move |e| {
                    if e.key() == Key::Enter {
                        if let Some(h) = on_enter {
                            h.call(());
                        }
                    }
                },
            }
        }
    }
}

/// The Kompass brand mark: an 8-point compass rose — an accent cardinal star
/// over a muted diagonal star. Two filled paths (self-colored, ignores
/// `currentColor`); sized by the caller's class.
fn kompass_mark(class: &str) -> Element {
    rsx! {
        svg { class: "{class}", "viewBox": "0 0 32 32", fill: "none",
            path {
                d: "M24.1 7.9 L18.6 16 L24.1 24.1 L16 18.6 L7.9 24.1 L13.4 16 L7.9 7.9 L16 13.4 Z",
                fill: "var(--fg-muted)",
            }
            path {
                d: "M16 2 L18.5 13.5 L30 16 L18.5 18.5 L16 30 L13.5 18.5 L2 16 L13.5 13.5 Z",
                fill: "var(--accent)",
            }
        }
    }
}

#[component]
fn App() -> Element {
    let prefs = use_hook(config::load);

    let mut rows = use_signal(Vec::<ResourceRow>::new);
    let mut conn = use_signal(|| ConnState::Connecting);
    let mut context = use_signal(|| "—".to_string());
    // Theme mode ("system"/"dark"/"light"); `os_dark` tracks the OS appearance
    // (updated from a prefers-color-scheme listener) for the "system" mode.
    let mut theme_mode = use_signal(|| prefs.theme.clone());
    let mut os_dark = use_signal(|| true);
    let cycle_theme = use_callback(move |_: ()| {
        let next = match theme_mode().as_str() {
            "system" => "light",
            "light" => "dark",
            _ => "system",
        };
        theme_mode.set(next.into());
    });
    // Boot to the user's default page ("overview" or a kind id).
    let boot_page = prefs.default_page.clone();
    let mut default_page = use_signal(|| prefs.default_page.clone());
    // Clusters pinned to the top of the switcher (persisted).
    let mut pinned = use_signal(|| prefs.pinned_clusters.clone());
    let mut kind = use_signal(|| {
        if boot_page == "overview" { "deployments.apps".to_string() } else { boot_page.clone() }
    });
    let mut catalog = use_signal(Vec::<KindMeta>::new);
    // Live usage: "namespace/name" → (cpu_milli, mem_bytes).
    let mut metrics = use_signal(std::collections::HashMap::<String, (i64, i64)>::new);
    // Usage history per key for sparklines: (cpu_series, mem_series).
    let mut metrics_hist =
        use_signal(std::collections::HashMap::<String, (Vec<i64>, Vec<i64>)>::new);
    // Expanded nav categories (the active one is always expanded).
    let mut expanded = use_signal(std::collections::HashSet::<String>::new);
    // Overview dashboard.
    let mut overview_on = use_signal(|| boot_page == "overview");
    let mut overview = use_signal(|| None::<OverviewData>);
    let mut events = use_signal(Vec::<EventRow>::new);
    // Pods owned by the controller shown in the detail panel (workloads only).
    let mut ctrl_pods = use_signal(Vec::<ResourceRow>::new);
    // Last metrics-enabled value sent to the engine (gates the poller).
    let mut metrics_sent = use_signal(|| false);
    // Pending delete confirmation (None = no dialog).
    let mut confirm = use_signal(|| None::<DeleteReq>);
    // Active port-forwards.
    let mut port_forwards = use_signal(Vec::<PortForward>::new);
    let mut pf_open = use_signal(|| false);
    // Table search term, kept per (kind id, namespace-view index) so it's
    // preserved as you switch resource types and namespace tabs.
    let mut queries = use_signal(std::collections::HashMap::<(String, usize), String>::new);
    let mut status_filter = use_signal(|| None::<String>);
    let mut status_open = use_signal(|| false);
    let mut columns_open = use_signal(|| false);
    // Hidden column keys per kind id.
    let mut col_hidden = use_signal(|| {
        prefs
            .columns
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect::<std::collections::HashSet<String>>()))
            .collect::<std::collections::HashMap<String, std::collections::HashSet<String>>>()
    });
    let mut selected = use_signal(BTreeSet::<String>::new);
    // Namespace views: each is a saved filter (None = all). Active one is the
    // dropdown; the others are tab buttons that switch the view. Restored from prefs.
    let mut ns_views = use_signal(|| {
        if prefs.ns_views.is_empty() {
            vec![prefs.namespace.clone()]
        } else {
            prefs.ns_views.clone()
        }
    });
    let mut ns_active = use_signal(|| prefs.ns_active.min(prefs.ns_views.len().max(1) - 1));

    let mut ns_open = use_signal(|| false);
    let mut ns_query = use_signal(String::new);
    let mut ns_hl = use_signal(|| 0usize);
    // Namespace views remembered per cluster (persisted), so switching clusters
    // — and restarts — restore each cluster's views.
    let mut nsv_by_ctx = use_signal(|| prefs.ns_views_by_ctx.clone());
    let ctx_menu = use_signal(|| None::<CtxMenu>);
    let mut nav_menu = use_signal(|| None::<NavMenu>);

    // Detail panel state.
    let mut detail = use_signal(|| None::<DetailTarget>);
    let mut detail_tab = use_signal(|| DetailTab::Summary);
    let detail_w = use_signal(|| 560.0_f64);
    let mut detail_full = use_signal(|| false);
    let mut manifest = use_signal(|| None::<String>);
    let mut manifest_err = use_signal(|| None::<String>);
    let mut logs = use_signal(Vec::<LogEntry>::new);
    let mut multi_logs = use_signal(|| 0usize); // 0 = closed, else selection count
    let mut multi_label = use_signal(String::new); // e.g. "2 deployments"
    let resize_start = use_signal(|| None::<(f64, f64)>);
    // Virtualization: current scroll offset + viewport height of the table.
    let mut scroll_top = use_signal(|| 0.0_f64);
    let mut viewport_h = use_signal(|| 900.0_f64);
    let mut toasts = use_signal(Vec::<Toast>::new);
    // Stale-while-revalidate cache: last rows per (context, kind) so switching
    // tabs/clusters paints instantly, then reconciles to live (no cold flash).
    let mut kind_cache =
        use_signal(std::collections::HashMap::<(String, String), Vec<ResourceRow>>::new);
    let mut resyncing = use_signal(|| false);
    let mut seen = use_signal(std::collections::HashSet::<String>::new);
    let mut sort = use_signal(|| (SortKey::from_id(&prefs.sort_key), prefs.sort_asc));

    // Boot navigation: engine starts on deployments.apps; steer to the default page.
    use_hook(|| {
        if boot_page == "overview" {
            send_cmd(Cmd::FetchOverview);
        } else if boot_page != "deployments.apps" {
            send_cmd(Cmd::SetKind(boot_page.clone()));
        }
    });

    // Scope the engine's watches to the active namespace view. Watching one
    // namespace is far cheaper than cluster-wide on big clusters (e.g. listing
    // every Secret across all namespaces), so this fixes slow-loading kinds.
    // "All namespaces" (None) keeps the cluster-wide watch. The engine handler
    // is idempotent, so re-sending on unrelated changes is harmless.
    use_effect(move || {
        let _ = context(); // re-send after a context switch (engine resets it)
        let active_ns = ns_views().get(ns_active()).cloned().flatten();
        send_cmd(Cmd::SetNamespace(active_ns));
    });

    // Persist preferences whenever the relevant inputs change.
    use_effect(move || {
        let (sk, sa) = sort();
        // Fold the active cluster's current views into the per-cluster map.
        let mut nsv_map = nsv_by_ctx();
        nsv_map.insert(context(), (ns_views(), ns_active()));
        config::save(&config::Prefs {
            context: context(),
            namespace: ns_views().get(ns_active()).cloned().flatten(),
            kind: kind(),
            theme: theme_mode(),
            sort_key: sk.id(),
            sort_asc: sa,
            ns_views: ns_views(),
            ns_active: ns_active(),
            columns: col_hidden()
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                .collect(),
            default_page: default_page(),
            pinned_clusters: pinned(),
            ns_views_by_ctx: nsv_map,
        });
    });
    let mut toast_seq = use_signal(|| 0u64);

    let platform = if cfg!(target_os = "macos") { "mac" } else { "other" };
    let win = dioxus::desktop::use_window();

    // macOS draws a 1px titlebar separator hairline at the window's top edge
    // (default `.automatic`). With our transparent fullsize-content titlebar it
    // reads as a stray white line — turn it off.
    #[cfg(target_os = "macos")]
    use_hook({
        let win = win.clone();
        move || {
            use dioxus::desktop::tao::platform::macos::WindowExtMacOS;
            use objc2_app_kit::{NSTitlebarSeparatorStyle, NSWindow};
            let ptr = win.ns_window() as *mut NSWindow;
            if !ptr.is_null() {
                let nswin: &NSWindow = unsafe { &*ptr };
                nswin.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
            }
        }
    });

    // Cluster switcher.
    let contexts = use_hook(list_contexts);
    let mut cluster_open = use_signal(|| false);
    let mut cluster_query = use_signal(String::new);
    let toggle_pin_cluster = use_callback(move |name: String| {
        let mut p = pinned.write();
        if let Some(i) = p.iter().position(|c| c == &name) {
            p.remove(i);
        } else {
            p.push(name);
        }
    });

    // About popup.
    let mut about_open = use_signal(|| false);
    // Update check: (new version, installed-via-brew) when one is available.
    let mut update_info = use_signal(|| None::<(String, bool)>);
    let mut update_dismissed = use_signal(|| false);
    let mut checking_update = use_signal(|| false);
    let mut update_checked = use_signal(|| false); // ≥1 check has completed

    // Run a release check (GitHub API), refreshing the signals. Re-shows the
    // banner when the available version changes. Reused by the daily loop and
    // the manual "About" check.
    let check_update = use_callback(move |_: ()| {
        if *checking_update.peek() {
            return;
        }
        spawn(async move {
            checking_update.set(true);
            let found = tokio::task::spawn_blocking(|| {
                let tag = latest_release_tag()?;
                is_newer(&tag, env!("CARGO_PKG_VERSION"))
                    .then(|| (tag.trim_start_matches('v').to_string(), installed_via_brew()))
            })
            .await
            .ok()
            .flatten();
            match found {
                Some((ver, brew)) => {
                    let changed = update_info.peek().as_ref().map(|(v, _)| v != &ver).unwrap_or(true);
                    if changed {
                        update_dismissed.set(false);
                    }
                    update_info.set(Some((ver, brew)));
                }
                None => update_info.set(None),
            }
            update_checked.set(true);
            checking_update.set(false);
        });
    });

    // Check at startup, then once a day while the app stays open.
    use_hook(move || {
        spawn(async move {
            loop {
                check_update.call(());
                tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
            }
        });
    });

    // ⌘K command palette.
    let mut palette_open = use_signal(|| false);
    let mut palette_query = use_signal(String::new);
    let mut palette_sel = use_signal(|| 0usize);

    // View history (full view = page + ns-view + namespace + filter) for ⌘[ / ⌘].
    let mut hist = use_signal(Vec::<ViewState>::new);
    let mut hist_idx = use_signal(|| 0usize);
    // Nav request from ⌘[ / ⌘] — applied by an effect (after switch_kind exists).
    let mut nav_tick = use_signal(|| 0u32);
    let mut nav_back = use_signal(|| true);

    // Load the vendored xterm.js + fit addon once (offline; sets window globals).
    use_hook(|| {
        dioxus::document::eval(XTERM_JS);
        dioxus::document::eval(XTERM_FIT_JS);
    });

    // Track the OS appearance for "system" theme mode (and react to live changes).
    use_hook(|| {
        spawn(async move {
            let mut eval = dioxus::document::eval(
                "const mq=window.matchMedia('(prefers-color-scheme: dark)');\
                 const s=()=>dioxus.send(mq.matches?'osdark':'oslight');\
                 s(); mq.addEventListener('change', s);",
            );
            while let Ok(m) = eval.recv::<String>().await {
                os_dark.set(m == "osdark");
            }
        });
    });

    // Live Overview: re-fetch the snapshot every 8s while it's open. The merge on
    // Delta::Overview keeps prior values visible, so refreshes don't flash skeletons.
    use_hook(|| {
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                if *overview_on.peek() {
                    send_cmd(Cmd::FetchOverview);
                }
            }
        });
    });

    // Container-square tooltips open downward, but in the scrollable table they'd
    // be clipped by the scroll container near its bottom edge. On hover, flip the
    // tooltip toward the interior (up when there isn't room below).
    use_hook(|| {
        dioxus::document::eval(
            r#"document.addEventListener('mouseover', function(e){
                 const w = e.target.closest && e.target.closest('.csq-wrap');
                 if (!w) return;
                 const sc = w.closest('.table-wrap');
                 if (!sc) { w.classList.remove('flip-up'); return; }
                 const r = w.getBoundingClientRect(), b = sc.getBoundingClientRect();
                 const below = b.bottom - r.bottom, above = r.top - b.top;
                 if (below < 96 && above > below) w.classList.add('flip-up');
                 else w.classList.remove('flip-up');
               });"#,
        );
    });

    // Window-level ⌘K / ⌃K listener — fires regardless of focus (a div onkeydown
    // only catches bubbled events, missing ⌘K when nothing in the app is focused).
    use_hook(|| {
        spawn(async move {
            let mut eval = dioxus::document::eval(
                r#"window.addEventListener('keydown', function(e){
                     if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
                       e.preventDefault(); dioxus.send('toggle');
                     } else if ((e.metaKey || e.ctrlKey) && e.key === '[') {
                       e.preventDefault(); dioxus.send('back');
                     } else if ((e.metaKey || e.ctrlKey) && e.key === ']') {
                       e.preventDefault(); dioxus.send('forward');
                     } else if ((e.metaKey || e.ctrlKey) && (e.key === 'r' || e.key === 'R')) {
                       e.preventDefault(); dioxus.send('refresh');
                     } else if ((e.metaKey || e.ctrlKey) && (e.key === 't' || e.key === 'T')) {
                       e.preventDefault(); dioxus.send('newview');
                     } else if ((e.metaKey || e.ctrlKey) && (e.key === 'f' || e.key === 'F')) {
                       const inp = document.querySelector('.yaml-toolbar .search input, .logs-toolbar .search input');
                       if (inp) { e.preventDefault(); inp.focus(); if (inp.select) inp.select(); }
                     } else if ((e.metaKey || e.ctrlKey) && (e.key === 'g' || e.key === 'G')) {
                       const sel = e.shiftKey ? '[data-tip="Previous"]' : '[data-tip="Next"]';
                       const btn = document.querySelector('.yaml-toolbar .icon-btn' + sel);
                       if (btn) { e.preventDefault(); btn.click(); }
                     }
                   });"#,
            );
            while let Ok(msg) = eval.recv::<String>().await {
                match msg.as_str() {
                    "toggle" => {
                        palette_query.set(String::new());
                        palette_sel.set(0);
                        palette_open.toggle();
                    }
                    "back" => {
                        nav_back.set(true);
                        nav_tick += 1;
                    }
                    "forward" => {
                        nav_back.set(false);
                        nav_tick += 1;
                    }
                    "refresh" => {
                        if overview_on() {
                            send_cmd(Cmd::FetchOverview);
                        } else {
                            send_cmd(Cmd::SetKind(kind()));
                        }
                    }
                    "newview" => {
                        let n = {
                            let mut v = ns_views.write();
                            v.push(None);
                            v.len() - 1
                        };
                        ns_active.set(n);
                        ns_query.set(String::new());
                        ns_hl.set(0);
                        ns_open.set(true);
                    }
                    _ => {}
                }
            }
        });
    });
    let switch_context = use_callback(move |name: String| {
        cluster_open.set(false);
        if context() == name {
            return;
        }
        let cur = context();
        // Remember this cluster's namespace views; restore the target's (or default).
        nsv_by_ctx.write().insert(cur.clone(), (ns_views(), ns_active()));
        kind_cache.write().insert((cur, kind()), rows());
        let cached = kind_cache.read().get(&(name.clone(), kind())).cloned().unwrap_or_default();
        let (tv, ta) = nsv_by_ctx
            .read()
            .get(&name)
            .cloned()
            .unwrap_or_else(|| (vec![None], 0));
        let ta = ta.min(tv.len().saturating_sub(1));
        send_cmd(Cmd::SetContext(name.clone()));
        context.set(name); // optimistic; engine confirms via Delta::Context
        ns_views.set(tv);
        ns_active.set(ta);
        detail.set(None);
        detail_full.set(false);
        selected.write().clear();
        rows.set(cached);
        // History is per-cluster: the namespace-view indices belong to this
        // context, so reset it (the record effect reseeds for the new context).
        hist.write().clear();
        hist_idx.set(0);
    });


    // Open the detail panel on a target + tab: reset view, fetch manifest,
    // start logs if the Logs tab was requested.
    let open_detail = move |req: OpenReq| {
        let OpenReq { target, tab } = req;
        manifest.set(None);
        manifest_err.set(None);
        logs.write().clear();
        events.write().clear();
        ctrl_pods.write().clear();
        detail_tab.set(tab);
        send_cmd(Cmd::FetchManifest {
            kind_id: target.kind_id.clone(),
            namespace: target.namespace.clone(),
            name: target.name.clone(),
        });
        // Workloads: list their pods for the Summary section.
        if is_workload(&target.kind_name) {
            send_cmd(Cmd::FetchControllerPods {
                kind_id: target.kind_id.clone(),
                namespace: target.namespace.clone(),
                name: target.name.clone(),
            });
        }
        if matches!(tab, DetailTab::Logs) {
            send_cmd(Cmd::StartLogs {
                kind_id: target.kind_id.clone(),
                namespace: target.namespace.clone(),
                name: target.name.clone(),
                container: None,
            });
        }
        if matches!(tab, DetailTab::Events) {
            send_cmd(Cmd::FetchEvents {
                namespace: target.namespace.clone(),
                name: target.name.clone(),
            });
        }
        detail.set(Some(target));
    };

    // Switch tabs within an open panel (start/stop the log stream accordingly).
    let switch_tab = move |tab: DetailTab| {
        if detail_tab() == tab {
            return;
        }
        if let Some(t) = detail() {
            match tab {
                DetailTab::Logs => send_cmd(Cmd::StartLogs {
                    kind_id: t.kind_id.clone(),
                    namespace: t.namespace.clone(),
                    name: t.name.clone(),
                    container: None,
                }),
                DetailTab::Events => {
                    send_cmd(Cmd::StopLogs);
                    events.write().clear();
                    send_cmd(Cmd::FetchEvents { namespace: t.namespace.clone(), name: t.name.clone() });
                }
                _ => send_cmd(Cmd::StopLogs),
            }
        }
        detail_tab.set(tab);
    };

    let close_detail = move |_| {
        send_cmd(Cmd::StopLogs);
        detail.set(None);
        detail_full.set(false);
    };

    // Bridge coroutine: drains engine deltas into signals. Runs on the Dioxus
    // executor; awaiting the tokio mpsc here is runtime-agnostic (no block).
    use_coroutine(move |_: UnboundedReceiver<()>| async move {
        let mut rx = RX.lock().unwrap().take().expect("receiver taken once");
        // Coalesce bursts: apply deltas synchronously without awaiting between
        // them so Dioxus re-renders once per batch, not once per delta. A cap
        // keeps a giant initial sync from blocking the UI for a whole frame.
        while let Some(first) = rx.recv().await {
            let mut delta = first;
            let mut n = 0u32;
            loop {
                match delta {
                Delta::Context(name) => context.set(name),
                Delta::Catalog(metas) => catalog.set(metas),
                Delta::Overview(d) => {
                    // Stale-while-revalidate: merge onto the prior snapshot so a
                    // live refresh updates values in place without flashing skeletons.
                    let merged = overview.peek().as_ref().map(|p| p.merge_from(&d)).unwrap_or(d);
                    overview.set(Some(merged));
                }
                Delta::PortForwards(v) => port_forwards.set(v),
                Delta::Events(v) => events.set(v),
                Delta::ControllerPods(v) => ctrl_pods.set(v),
                Delta::ScopedNamespace(ns) => {
                    // No cluster-wide access — lock the view to the one namespace
                    // the connection can list (from the kubeconfig context).
                    ns_views.set(vec![Some(ns.clone())]);
                    ns_active.set(0);
                    let id = toast_seq() + 1;
                    toast_seq.set(id);
                    toasts.write().push(Toast {
                        id,
                        ok: true,
                        msg: format!("No cluster-wide access — showing namespace “{ns}”"),
                    });
                }
                Delta::Metrics(samples) => {
                    let mut m = metrics.write();
                    let mut h = metrics_hist.write();
                    m.clear();
                    for s in samples {
                        let key = format!("{}/{}", s.namespace, s.name);
                        m.insert(key.clone(), (s.cpu_milli, s.mem_bytes));
                        let (cpu, mem) = h.entry(key).or_default();
                        cpu.push(s.cpu_milli);
                        mem.push(s.mem_bytes);
                        // Cap history (~7 min at 10s polling).
                        if cpu.len() > 40 {
                            cpu.remove(0);
                            mem.remove(0);
                        }
                    }
                }
                Delta::Conn(state) => {
                    let live = matches!(state, ConnState::Live);
                    conn.set(state);
                    // End of a (re)sync: drop rows not seen this round (handles
                    // deletes that happened while we showed cached data).
                    if live && resyncing() {
                        let keep = seen();
                        rows.write()
                            .retain(|r| keep.contains(&format!("{}/{}", r.namespace, r.name)));
                        resyncing.set(false);
                    }
                }
                // Don't clear: keep showing cached rows, reconcile on Live.
                Delta::Reset => {
                    resyncing.set(true);
                    seen.write().clear();
                }
                Delta::Applied { kind_id: k, row } => {
                    // Ignore late deltas from a previous kind's watch.
                    if k == kind() {
                        let key = format!("{}/{}", row.namespace, row.name);
                        {
                            let mut list = rows.write();
                            match list
                                .iter_mut()
                                .find(|r| r.namespace == row.namespace && r.name == row.name)
                            {
                                Some(existing) => *existing = row,
                                None => list.push(row),
                            }
                        }
                        if resyncing() {
                            seen.write().insert(key);
                        }
                    }
                }
                Delta::Deleted { kind_id: k, namespace, name } => {
                    if k == kind() {
                        rows.write()
                            .retain(|r| !(r.namespace == namespace && r.name == name));
                    }
                }
                Delta::Manifest { namespace, name, yaml } => {
                    // Only apply if it matches the currently open target.
                    if detail().is_some_and(|t| t.namespace == namespace && t.name == name) {
                        manifest_err.set(None);
                        manifest.set(Some(yaml));
                    }
                }
                Delta::ManifestErr { namespace, name, error } => {
                    if detail().is_some_and(|t| t.namespace == namespace && t.name == name) {
                        manifest_err.set(Some(error));
                    }
                }
                Delta::LogReset => logs.write().clear(),
                Delta::LogLine { source, idx, line } => {
                    let mut l = logs.write();
                    l.push(LogEntry { source, idx, line });
                    // Bounded buffer — protect memory under high volume.
                    if l.len() > 2000 {
                        let drop = l.len() - 2000;
                        l.drain(0..drop);
                    }
                }
                Delta::LogEnd => {}
                Delta::ExecReset => {
                    dioxus::document::eval("window.__kterm && window.__kterm.reset();");
                }
                Delta::ExecData(s) => {
                    let json = serde_json::to_string(&s).unwrap_or_else(|_| "\"\"".into());
                    dioxus::document::eval(&format!("window.__kterm && window.__kterm.write({json});"));
                }
                Delta::ExecEnd => {
                    dioxus::document::eval(
                        "window.__kterm && window.__kterm.write('\\r\\n\\x1b[90m[session ended]\\x1b[0m\\r\\n');",
                    );
                }
                Delta::ActionResult { ok, message } => {
                    let id = toast_seq() + 1;
                    toast_seq.set(id);
                    let mut t = toasts.write();
                    t.push(Toast { id, ok, msg: message });
                    // Keep only the most recent few.
                    let len = t.len();
                    if len > 4 {
                        t.drain(0..len - 4);
                    }
                }
                }
                // Drain any already-queued deltas before yielding, so a burst
                // collapses into a single re-render. Cap keeps frames flowing.
                n += 1;
                if n >= 800 {
                    break;
                }
                match rx.try_recv() {
                    Ok(d) => delta = d,
                    Err(_) => break,
                }
            }
        }
    });

    // Derived view state.
    // Effective light/dark: explicit modes win; "system" follows the OS.
    let theme_dark = match theme_mode().as_str() {
        "light" => false,
        "dark" => true,
        _ => os_dark(),
    };
    let theme = if theme_dark { "dark" } else { "light" };
    let ctx = context();
    let accent_var = format!(
        "--cluster-accent: var(--kc-{});",
        cluster_accent_index(&ctx)
    );

    let all_rows = rows();
    // Distinct namespaces for the namespace dropdown.
    let namespaces: BTreeSet<String> = all_rows.iter().map(|r| r.namespace.clone()).collect();
    // Distinct statuses (status, class) for the status filter dropdown.
    let mut statuses: Vec<(String, String)> = Vec::new();
    for r in all_rows.iter() {
        if !statuses.iter().any(|(s, _)| s == &r.status) {
            statuses.push((r.status.clone(), r.status_class.clone()));
        }
    }
    statuses.sort_by(|a, b| a.0.cmp(&b.0));
    let active_status = status_filter();
    let active_ns = ns_views().get(ns_active()).cloned().flatten();

    // Add a namespace view (all-namespaces) and open its picker — shared by the
    // "+" button and the ⌘T shortcut.
    let add_ns_view = use_callback(move |_: ()| {
        let n = {
            let mut v = ns_views.write();
            v.push(None);
            v.len() - 1
        };
        ns_active.set(n);
        ns_query.set(String::new());
        ns_hl.set(0);
        ns_open.set(true);
    });

    // Set the active view's namespace (used by the dropdown).
    let set_active_ns = use_callback(move |ns: Option<String>| {
        let i = ns_active();
        let mut v = ns_views.write();
        if i < v.len() {
            v[i] = ns;
        }
    });

    // Remove a namespace view tab (keeps at least one).
    let remove_view = use_callback(move |i: usize| {
        ns_open.set(false);
        let len = ns_views.read().len();
        if len <= 1 {
            return;
        }
        ns_views.write().remove(i);
        let a = ns_active();
        let new_active = if a == i {
            i.min(len - 2)
        } else if a > i {
            a - 1
        } else {
            a
        };
        ns_active.set(new_active);
    });

    let query_key = (kind(), ns_active());
    let query_val = queries().get(&query_key).cloned().unwrap_or_default();
    let q = query_val.to_lowercase();
    let mut view: Vec<ResourceRow> = all_rows
        .into_iter()
        .filter(|r| active_ns.as_ref().is_none_or(|ns| &r.namespace == ns))
        .filter(|r| active_status.as_ref().is_none_or(|s| &r.status == s))
        .filter(|r| {
            q.is_empty()
                || r.name.to_lowercase().contains(&q)
                || r.namespace.to_lowercase().contains(&q)
        })
        .collect();
    // Live usage map ("ns/name" → (cpu_milli, mem_bytes)) — also drives CPU/Mem sort.
    let metric_map = metrics();
    let (sort_key, sort_asc) = sort();
    // Usage for a row, or -1 when no sample yet (sorts unknowns last when desc).
    let usage = |r: &ResourceRow, mem: bool| -> i64 {
        metric_map
            .get(&format!("{}/{}", r.namespace, r.name))
            .map(|(c, m)| if mem { *m } else { *c })
            .unwrap_or(-1)
    };
    view.sort_by(|a, b| {
        let ord = match sort_key {
            SortKey::Name => a.name.cmp(&b.name),
            SortKey::Namespace => a.namespace.cmp(&b.namespace).then_with(|| a.name.cmp(&b.name)),
            SortKey::Status => a.status.cmp(&b.status).then_with(|| a.name.cmp(&b.name)),
            SortKey::Age => parse_age_secs(&a.age).cmp(&parse_age_secs(&b.age)),
            SortKey::Col(i) => col_cmp(
                a.cols.get(i).map(String::as_str).unwrap_or(""),
                b.cols.get(i).map(String::as_str).unwrap_or(""),
            ),
            SortKey::Cpu => usage(a, false).cmp(&usage(b, false)).then_with(|| a.name.cmp(&b.name)),
            SortKey::Mem => usage(a, true).cmp(&usage(b, true)).then_with(|| a.name.cmp(&b.name)),
        };
        if sort_asc { ord } else { ord.reverse() }
    });
    let total = view.len();

    // Toggle sort on header click (same column flips direction).
    let sort_click = use_callback(move |k: SortKey| {
        let (cur, asc) = sort();
        if cur == k {
            sort.set((k, !asc));
        } else {
            sort.set((k, true));
        }
    });
    let sort_ind = move |k: SortKey| -> &'static str {
        let (cur, asc) = sort();
        if cur == k {
            if asc { " ▲" } else { " ▼" }
        } else {
            ""
        }
    };


    let active_id = kind();
    let cat = catalog();
    let active_meta = cat.iter().find(|m| m.id() == active_id).cloned();
    let kind_name = active_meta.as_ref().map(|m| m.kind.clone()).unwrap_or_else(|| "Pod".into());
    // Before the catalog loads there's no meta yet — derive a clean label from
    // the id (plural before the group) instead of showing the raw "plural.group".
    let title = active_meta
        .as_ref()
        .map(|m| m.label())
        .unwrap_or_else(|| label_from_id(&active_id));
    let kind_cols = columns_for(&kind_name);
    let show_metrics = has_metrics(&kind_name);
    let metric_hist = metrics_hist();

    // Column visibility for the active kind (namespace hidden by default).
    let hidden: std::collections::HashSet<String> = col_hidden()
        .get(&active_id)
        .cloned()
        .unwrap_or_else(|| ["namespace".to_string()].into_iter().collect());
    let vis = ColVis {
        ns: !hidden.contains("namespace"),
        status: !hidden.contains("status"),
        age: !hidden.contains("age"),
        cpu: show_metrics && !hidden.contains("cpu"),
        mem: show_metrics && !hidden.contains("mem"),
        cols: (0..kind_cols.len()).map(|i| !hidden.contains(&format!("col:{i}"))).collect(),
    };
    // Options shown in the Columns dialog (key, label).
    let mut col_opts: Vec<(String, String)> = vec![
        ("namespace".into(), "Namespace".into()),
        ("status".into(), "Status".into()),
    ];
    for (i, c) in kind_cols.iter().enumerate() {
        col_opts.push((format!("col:{i}"), c.to_string()));
    }
    if show_metrics {
        col_opts.push(("cpu".into(), "CPU".into()));
        col_opts.push(("mem".into(), "Mem".into()));
    }
    col_opts.push(("age".into(), "Age".into()));
    let hidden_for_dialog = hidden.clone();

    // Only poll metrics when a view actually shows them (CPU/Mem column or Overview).
    let metrics_wanted = overview_on() || (vis.cpu || vis.mem);
    if *metrics_sent.peek() != metrics_wanted {
        metrics_sent.set(metrics_wanted);
        send_cmd(Cmd::SetMetrics(metrics_wanted));
    }

    // Toggle a column key for the active kind.
    let toggle_col = use_callback(move |key: String| {
        let id = kind();
        let mut m = col_hidden.write();
        let set = m.entry(id).or_insert_with(|| ["namespace".to_string()].into_iter().collect());
        if !set.remove(&key) {
            set.insert(key);
        }
    });
    let active_cat_name = active_meta.as_ref().map(|m| m.category()).unwrap_or("Workloads");


    // Virtualization window: only render rows near the viewport.
    let st = scroll_top();
    let vp = viewport_h();
    let vis_start = ((st / ROW_H).floor() as usize).saturating_sub(OVERSCAN);
    let vis_end = (((st + vp) / ROW_H).ceil() as usize + OVERSCAN).min(total);
    let vis_start = vis_start.min(vis_end);
    let top_pad = vis_start as f64 * ROW_H;
    let bot_pad = total.saturating_sub(vis_end) as f64 * ROW_H;
    let col_span = 4 + kind_cols.len() + 1;

    // Switch the active resource kind: cache current rows, paint the cached
    // rows for the new kind instantly, then re-watch to revalidate.
    let switch_kind = use_callback(move |id: String| {
        overview_on.set(false);
        if kind() == id {
            return;
        }
        send_cmd(Cmd::StopLogs);
        detail.set(None);
        detail_full.set(false);
        selected.write().clear();
        // Search term is kept per (kind, ns-view); the new kind shows its own.
        let ctx = context();
        kind_cache.write().insert((ctx.clone(), kind()), rows());
        let cached = kind_cache.read().get(&(ctx, id.clone())).cloned().unwrap_or_default();
        rows.set(cached);
        kind.set(id.clone());
        send_cmd(Cmd::SetKind(id));
    });

    // One-click: jump to the Pods view filtered to a controller's name.
    let view_pods_for = use_callback(move |name: String| {
        queries.write().insert(("pods".to_string(), ns_active()), name);
        switch_kind.call("pods".to_string());
    });

    // Record a history entry whenever the *place* changes — page (overview vs a
    // kind), the namespace-view tab, or that tab's namespace. Namespace changes
    // (dropdown, in place) and Overview are now navigable. The filter is only a
    // snapshot here; typing alone never pushes a new entry (kept in sync below).
    use_effect(move || {
        let k = kind();
        let a = ns_active();
        let cur = ViewState {
            overview: overview_on(),
            kind: k.clone(),
            ns_active: a,
            namespace: ns_views().get(a).cloned().flatten(),
            filter: queries.peek().get(&(k, a)).cloned().unwrap_or_default(),
        };
        let mut h = hist.write();
        let i = *hist_idx.peek();
        if h.is_empty() {
            h.push(cur);
            drop(h);
            hist_idx.set(0);
            return;
        }
        if h[i].same_place(&cur) {
            return;
        }
        h.truncate(i + 1);
        h.push(cur);
        let n = h.len();
        drop(h);
        hist_idx.set(n - 1);
    });

    // Keep the current entry's filter in sync as the user types — updates the
    // entry in place (no new history entry) so back/forward restores the filter.
    use_effect(move || {
        let k = kind();
        let a = ns_active();
        let f = queries().get(&(k.clone(), a)).cloned().unwrap_or_default();
        let i = *hist_idx.peek();
        let mut h = hist.write();
        if let Some(e) = h.get_mut(i) {
            if e.kind == k && e.ns_active == a && e.overview == *overview_on.peek() && e.filter != f {
                e.filter = f;
            }
        }
    });

    // Scroll shadow: cast a shadow under the pinned About footer onto the nav
    // list, but only while the list overflows and there's content below the fold.
    use_effect(move || {
        dioxus::document::eval(
            "if(!window.__navShadow){window.__navShadow=1;\
             const f=()=>{const sc=document.querySelector('.nav-scroll'),nv=document.querySelector('.nav');\
             if(!sc||!nv)return;const more=sc.scrollTop+sc.clientHeight<sc.scrollHeight-1;\
             nv.classList.toggle('scroll-below',more);};\
             const w=()=>{const sc=document.querySelector('.nav-scroll');if(!sc){setTimeout(w,80);return;}\
             sc.addEventListener('scroll',f,{passive:true});new ResizeObserver(f).observe(sc);\
             new MutationObserver(f).observe(sc,{childList:true,subtree:true,attributes:true});f();};w();}",
        );
    });

    // Apply ⌘[ / ⌘] navigation (peek everything but the tick to avoid loops).
    use_effect(move || {
        if nav_tick() == 0 {
            return;
        }
        let back = *nav_back.peek();
        let h = hist.peek().clone();
        let i = *hist_idx.peek();
        let ni = if back {
            if i == 0 { return; }
            i - 1
        } else {
            if i + 1 >= h.len() { return; }
            i + 1
        };
        let v = h[ni].clone();
        hist_idx.set(ni);

        // Page: switch kind first (it resets overview_on), then set the flag.
        if *kind.peek() != v.kind {
            switch_kind.call(v.kind.clone());
        }
        overview_on.set(v.overview);

        // Namespace-view tab + restore that tab's namespace.
        let idx = v.ns_active.min(ns_views.peek().len().saturating_sub(1));
        {
            let mut nv = ns_views.write();
            if idx < nv.len() {
                nv[idx] = v.namespace.clone();
            }
        }
        ns_active.set(idx);

        // Filter for this view.
        let key = (v.kind.clone(), idx);
        if v.filter.is_empty() {
            queries.write().remove(&key);
        } else {
            queries.write().insert(key, v.filter.clone());
        }
    });

    // Delete request: Pods delete immediately; everything else confirms first.
    let run_delete = use_callback(move |req: DeleteReq| {
        for c in req.cmds {
            send_cmd(c);
        }
        if req.close_detail {
            detail.set(None);
            detail_full.set(false);
        }
    });
    let ask_delete = use_callback(move |req: DeleteReq| {
        if req.is_pod {
            run_delete.call(req);
        } else {
            confirm.set(Some(req));
        }
    });

    // Set (or clear → back to deployments) the default boot page.
    let set_default = use_callback(move |page: String| {
        if default_page() == page {
            default_page.set("deployments.apps".into());
        } else {
            default_page.set(page);
        }
    });

    // Run a palette action, then close the palette.
    let dispatch = use_callback(move |a: PalAction| {
        match a {
            PalAction::Kind(id) => switch_kind.call(id),
            PalAction::Context(c) => switch_context.call(c),
            PalAction::ToggleTheme => cycle_theme.call(()),
            PalAction::Open(t) => {
                manifest.set(None);
                manifest_err.set(None);
                logs.write().clear();
                detail_tab.set(DetailTab::Summary);
                send_cmd(Cmd::FetchManifest {
                    kind_id: t.kind_id.clone(),
                    namespace: t.namespace.clone(),
                    name: t.name.clone(),
                });
                detail.set(Some(t));
            }
        }
        palette_open.set(false);
    });

    // ⌘K palette results (commands, kinds, clusters, matching resources).
    let mut pal_items: Vec<PalItem> = Vec::new();
    if palette_open() {
        let pq = palette_query().to_lowercase();
        let (theme_icon, theme_title) = match theme_mode().as_str() {
            "system" => ("i-display", "Theme: System → Light"),
            "light" => ("i-sun", "Theme: Light → Dark"),
            _ => ("i-moon", "Theme: Dark → System"),
        };
        if pq.is_empty() || theme_title.to_lowercase().contains(&pq) || "theme".contains(&pq) {
            pal_items.push(PalItem {
                group: "Commands",
                icon: theme_icon,
                title: theme_title.into(),
                sub: String::new(),
                action: PalAction::ToggleTheme,
            });
        }
        for m in cat.iter() {
            let label = m.label();
            if pq.is_empty() || label.to_lowercase().contains(&pq) {
                pal_items.push(PalItem {
                    group: "Kinds",
                    icon: "i-workloads",
                    title: label,
                    sub: m.category().to_string(),
                    action: PalAction::Kind(m.id()),
                });
            }
        }
        for c in contexts.iter() {
            if pq.is_empty() || c.to_lowercase().contains(&pq) {
                pal_items.push(PalItem {
                    group: "Clusters",
                    icon: "i-compass",
                    title: c.clone(),
                    sub: "context".into(),
                    action: PalAction::Context(c.clone()),
                });
            }
        }
        if !pq.is_empty() {
            for r in rows().iter().filter(|r| r.name.to_lowercase().contains(&pq)).take(8) {
                pal_items.push(PalItem {
                    group: "Resources",
                    icon: "i-summary",
                    title: r.name.clone(),
                    sub: format!("{} · {}", kind_name, r.namespace),
                    action: PalAction::Open(DetailTarget {
                        kind_id: active_id.clone(),
                        kind_name: kind_name.clone(),
                        namespace: r.namespace.clone(),
                        name: r.name.clone(),
                        status: r.status.clone(),
                        status_class: r.status_class.clone(),
                        cols: r.cols.clone(),
                        age: r.age.clone(),
                    }),
                });
            }
        }
    }
    let pal_count = pal_items.len();
    let pal_sel = palette_sel().min(pal_count.saturating_sub(1));
    let pal_for_enter = pal_items.clone();

    let conn_state = conn();
    let has_rows = !view.is_empty();
    let cold = matches!(conn_state, ConnState::Connecting) && !has_rows;
    let revalidating = matches!(conn_state, ConnState::Connecting) && has_rows;
    let errored = matches!(conn_state, ConnState::Error(_));
    let empty = matches!(conn_state, ConnState::Live) && !has_rows;

    // Freshness indicator (top-right).
    let (fresh_class, fresh_label, fresh_tip) = match &conn_state {
        ConnState::Live => ("freshness", "Live", "Live watch active".to_string()),
        ConnState::Connecting if has_rows => (
            "freshness revalidating",
            "Revalidating",
            "Refreshing from cluster…".to_string(),
        ),
        ConnState::Connecting => (
            "freshness revalidating",
            "Connecting",
            "Connecting to cluster…".to_string(),
        ),
        ConnState::Error(e) => ("freshness error", "Stale", conn_error_tip(e)),
    };

    let sel_count = selected().len();
    let wrap_class = if revalidating {
        "table-wrap is-revalidating"
    } else {
        "table-wrap"
    };

    rsx! {
        style { dangerous_inner_html: "{TOKENS_CSS}\n{APP_CSS}\n{DETAIL_CSS}\n{OVERLAYS_CSS}\n{SCREENS_CSS}\n{CONTAINERS_CSS}\n{NAV_CSS}\n{XTERM_CSS}\n{EXTRA_CSS}" }
        div { dangerous_inner_html: "{SPRITE}", style: "position:absolute;width:0;height:0" }

        div {
            class: "app",
            "data-theme": "{theme}",
            "data-platform": "{platform}",
            style: "{accent_var}",
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    palette_open.set(false);
                }
            },
            div { class: "cluster-hairline" }

            // Reserved OS traffic-light space over the sidebar (drag band).
            div {
                class: "titlebar-pad",
                onmousedown: {
                    let w = win.clone();
                    move |_| { let _ = w.drag(); }
                },
                ondoubleclick: {
                    let w = win.clone();
                    move |_| w.set_maximized(!w.is_maximized())
                },
            }

            // ===== Top bar (content column; leads with cluster) =====
            header {
                class: "topbar",
                // Native window drag / double-click maximize (WKWebView ignores
                // -webkit-app-region, so we drive the window API directly).
                onmousedown: {
                    let w = win.clone();
                    move |_| { let _ = w.drag(); }
                },
                ondoubleclick: {
                    let w = win.clone();
                    move |_| w.set_maximized(!w.is_maximized())
                },
                div { class: "cluster-wrap", onmousedown: move |e| e.stop_propagation(),
                    button {
                        class: "cluster-select",
                        onclick: move |_| { cluster_query.set(String::new()); cluster_open.toggle(); },
                        span { class: "cluster-dot" }
                        span { class: "cluster-name", "{ctx}" }
                        span { class: "chev", {icon("i-chev-updown", "2")} }
                    }
                    if cluster_open() {
                        {
                            let cq = cluster_query().to_lowercase();
                            let pset = pinned();
                            let filtered: Vec<String> = contexts
                                .iter()
                                .filter(|c| cq.is_empty() || c.to_lowercase().contains(&cq))
                                .cloned()
                                .collect();
                            // Pinned first (in pin order), then the rest.
                            let pinned_list: Vec<String> =
                                pset.iter().filter(|c| filtered.contains(c)).cloned().collect();
                            let rest: Vec<String> =
                                filtered.iter().filter(|c| !pset.contains(c)).cloned().collect();
                            let has_sep = !pinned_list.is_empty() && !rest.is_empty();
                            let on_pick = move |name: String| switch_context.call(name);
                            let on_pin = move |name: String| toggle_pin_cluster.call(name);
                            rsx! {
                                div { class: "menu-scrim", onclick: move |_| cluster_open.set(false) }
                                div { class: "menu under",
                                    label { class: "menu-search",
                                        {icon("i-search", "1.8")}
                                        input {
                                            r#type: "text",
                                            placeholder: "Switch cluster…",
                                            autofocus: true,
                                            autocomplete: "off",
                                            autocapitalize: "off",
                                            spellcheck: "false",
                                            "autocorrect": "off",
                                            value: "{cluster_query}",
                                            oninput: move |e| cluster_query.set(e.value()),
                                        }
                                    }
                                    div { class: "menu-scroll",
                                        if filtered.is_empty() {
                                            div { class: "menu-empty", "No clusters match" }
                                        }
                                        for c in pinned_list.clone() {
                                            ClusterRow { name: c.clone(), active: c == ctx, pinned: true, on_pick, on_pin }
                                        }
                                        if has_sep {
                                            div { class: "menu-sep" }
                                        }
                                        for c in rest.clone() {
                                            ClusterRow { name: c.clone(), active: c == ctx, pinned: false, on_pick, on_pin }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "topbar-divider" }
                span { class: "ns-eyebrow", "ns" }
                div { class: "ns-tabs", onmousedown: move |e| e.stop_propagation(),
                    {
                        let nview_count = ns_views().len();
                        rsx! {
                    for (i, v) in ns_views().into_iter().enumerate() {
                        {
                            let is_active_tab = i == ns_active();
                            let vlabel = v.clone().unwrap_or_else(|| "all namespaces".into());
                            let multi = nview_count > 1;
                            if is_active_tab {
                                rsx! {
                                    div { class: "ns-wrap",
                                        button {
                                            class: if multi { "ns-select active removable" } else { "ns-select active" },
                                            onclick: move |_| { ns_query.set(String::new()); ns_hl.set(0); ns_open.toggle(); },
                                            span { class: "val", "{vlabel}" }
                                            span { class: "chev", {icon("i-chev-down", "2")} }
                                            if multi {
                                                span {
                                                    class: "ns-close",
                                                    onclick: move |e| { e.stop_propagation(); remove_view.call(i); },
                                                    {icon("i-x", "2.4")}
                                                }
                                            }
                                        }
                                        if ns_open() {
                                            {
                                                let nsq = ns_query().to_lowercase();
                                                let matches: Vec<String> = namespaces
                                                    .iter()
                                                    .filter(|ns| nsq.is_empty() || ns.to_lowercase().contains(&nsq))
                                                    .cloned()
                                                    .collect();
                                                let show_all = nsq.is_empty();
                                                let hl_idx = ns_hl().min(matches.len().saturating_sub(1));
                                                let matches_kd = matches.clone();
                                                rsx! {
                                                    div { class: "menu-scrim", onclick: move |_| ns_open.set(false) }
                                                    div { class: "menu under",
                                                        label { class: "menu-search",
                                                            {icon("i-search", "1.8")}
                                                            input {
                                                                r#type: "text",
                                                                placeholder: "Filter namespaces…",
                                                                autofocus: true,
                                                                autocomplete: "off",
                                                                autocapitalize: "off",
                                                                spellcheck: "false",
                                                                "autocorrect": "off",
                                                                value: "{ns_query}",
                                                                onmounted: move |e| { let _ = e.set_focus(true); },
                                                                oninput: move |e| { ns_query.set(e.value()); ns_hl.set(0); },
                                                                onkeydown: move |e| {
                                                                    let len = matches_kd.len();
                                                                    match e.key() {
                                                                        Key::Escape => ns_open.set(false),
                                                                        Key::ArrowDown => {
                                                                            e.prevent_default();
                                                                            if len > 0 { ns_hl.set((ns_hl() + 1).min(len - 1)); }
                                                                        }
                                                                        Key::ArrowUp => {
                                                                            e.prevent_default();
                                                                            ns_hl.set(ns_hl().saturating_sub(1));
                                                                        }
                                                                        Key::Enter => {
                                                                            if let Some(m) = matches_kd.get(ns_hl().min(len.saturating_sub(1))).cloned() {
                                                                                set_active_ns.call(Some(m));
                                                                                ns_open.set(false);
                                                                            }
                                                                        }
                                                                        _ => {}
                                                                    }
                                                                },
                                                            }
                                                        }
                                                        div { class: "menu-scroll",
                                                            if show_all {
                                                                div {
                                                                    class: "menu-item",
                                                                    onclick: move |_| { set_active_ns.call(None); ns_open.set(false); },
                                                                    span { "All namespaces" }
                                                                    if active_ns.is_none() {
                                                                        span { class: "check", {icon("i-check", "3")} }
                                                                    }
                                                                }
                                                                div { class: "menu-sep" }
                                                            }
                                                            if matches.is_empty() {
                                                                div { class: "menu-empty", "No namespaces match" }
                                                            }
                                                            for (j, ns) in matches.iter().cloned().enumerate() {
                                                                {
                                                                    let is_sel = active_ns.as_deref() == Some(ns.as_str());
                                                                    let class = if j == hl_idx { "menu-item hl" } else { "menu-item" };
                                                                    let pick = ns.clone();
                                                                    rsx! {
                                                                        div {
                                                                            class: "{class}",
                                                                            onclick: move |_| { set_active_ns.call(Some(pick.clone())); ns_open.set(false); },
                                                                            span { "{ns}" }
                                                                            if is_sel {
                                                                                span { class: "check", {icon("i-check", "3")} }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                rsx! {
                                    div {
                                        class: "ns-select ns-tab removable",
                                        onclick: move |_| { ns_open.set(false); ns_active.set(i); },
                                        span { class: "val", "{vlabel}" }
                                        span {
                                            class: "ns-close",
                                            onclick: move |e| { e.stop_propagation(); remove_view.call(i); },
                                            {icon("i-x", "2.4")}
                                        }
                                    }
                                }
                            }
                        }
                    }
                        }
                    }
                    button {
                        class: "ns-add tip",
                        "data-tip": "Add namespace view  ⌘T",
                        onclick: move |_| add_ns_view.call(()),
                        {icon("i-plus", "2")}
                    }
                }

                div { class: "topbar-spacer" }

                div { class: "topbar-right", onmousedown: move |e| e.stop_propagation(),
                    {
                        let fwds = port_forwards();
                        if !fwds.is_empty() {
                            rsx! {
                                div { class: "cluster-wrap",
                                    button {
                                        class: "kbar-hint",
                                        onclick: move |_| pf_open.toggle(),
                                        {icon("i-network", "1.8")}
                                        span { "{fwds.len()} forwarding" }
                                    }
                                    if pf_open() {
                                        div { class: "menu-scrim", onclick: move |_| pf_open.set(false) }
                                        div { class: "menu under", style: "left:auto; right:0; min-width:260px",
                                            div { class: "menu-head", "Port forwards" }
                                            for pf in fwds.iter().cloned() {
                                                {
                                                    let lp = pf.local_port;
                                                    rsx! {
                                                        div { class: "pf-row", style: "padding: var(--sp-3) var(--sp-4)",
                                                            span { class: "pf-fwd",
                                                                span { class: "d" }
                                                                "localhost:{pf.local_port} → {pf.name}:{pf.pod_port}"
                                                            }
                                                            button {
                                                                class: "btn btn-ghost btn-danger",
                                                                onclick: move |_| send_cmd(Cmd::StopPortForward { local_port: lp }),
                                                                "Stop"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            rsx! {}
                        }
                    }
                    button {
                        class: "kbar-hint",
                        onclick: move |_| { palette_query.set(String::new()); palette_sel.set(0); palette_open.set(true); },
                        {icon("i-search", "1.8")}
                        span { "Search" }
                        span { class: "kbd-row",
                            span { class: "kbd", "⌘" }
                            span { class: "kbd", "K" }
                        }
                    }
                    div { class: "{fresh_class} tip", "data-tip": "{fresh_tip}",
                        span { class: "pip" }
                        span { "{fresh_label}" }
                    }
                    button {
                        class: "icon-btn tip",
                        "data-tip": match theme_mode().as_str() {
                            "system" => "Theme: System (following OS)",
                            "light" => "Theme: Light",
                            _ => "Theme: Dark",
                        },
                        onclick: move |_| cycle_theme.call(()),
                        match theme_mode().as_str() {
                            "system" => rsx! { {icon("i-display", "1.7")} },
                            "light" => rsx! { {icon("i-sun", "1.7")} },
                            _ => rsx! { {icon("i-moon", "1.7")} },
                        }
                    }
                    // Windows/Linux only (hidden on macOS via data-platform).
                    div { class: "win-controls",
                        button {
                            class: "icon-btn",
                            onclick: {
                                let w = win.clone();
                                move |_| w.set_minimized(true)
                            },
                            {icon("i-chev-down", "2")}
                        }
                        button {
                            class: "icon-btn",
                            onclick: {
                                let w = win.clone();
                                move |_| w.set_maximized(!w.is_maximized())
                            },
                            {icon("i-panel", "1.7")}
                        }
                        button {
                            class: "icon-btn",
                            onclick: {
                                let w = win.clone();
                                move |_| w.close()
                            },
                            {icon("i-x", "1.8")}
                        }
                    }
                }
            }

            // ===== Left nav rail (carries the brand header) =====
            nav { class: "nav",
                div { class: "sidebar-brand",
                    {kompass_mark("brand-mark")}
                    span { class: "brand-word", "Kompass" }
                }
                div { class: "nav-scroll",
                div { class: "nav-eyebrow", "Cluster" }
                {
                    let ov_default = default_page() == "overview";
                    rsx! {
                        div {
                            class: if overview_on() { "nav-item active" } else { "nav-item" },
                            onclick: move |_| { overview_on.set(true); send_cmd(Cmd::FetchOverview); },
                            oncontextmenu: move |e| {
                                e.prevent_default();
                                let p = e.client_coordinates();
                                nav_menu.set(Some(NavMenu { x: p.x, y: p.y, key: "overview".into(), label: "Overview".into(), is_default: ov_default }));
                            },
                            {icon("i-overview", "1.7")}
                            span { class: "label", "Overview" }
                            span { class: "nav-right",
                                if ov_default {
                                    span {
                                        class: "def-pin tip settling", "data-tip": "Opens on launch",
                                        {icon("i-home", "1.9")}
                                    }
                                } else {
                                    button {
                                        class: "set-pin tip", "data-tip": "Set as default page", tabindex: "-1",
                                        onclick: move |e| { e.stop_propagation(); set_default.call("overview".to_string()); },
                                        {icon("i-home", "1.7")}
                                    }
                                }
                            }
                        }
                    }
                }
                for (icon_id, label) in CATS.iter() {
                    {
                        let mut kinds_in: Vec<KindMeta> =
                            cat.iter().filter(|m| m.category() == *label).cloned().collect();
                        kinds_in.sort_by(|a, b| a.label().cmp(&b.label()));
                        let has = !kinds_in.is_empty();
                        let is_active_cat = !overview_on() && *label == active_cat_name && active_meta.is_some();
                        let is_open = is_active_cat || expanded().contains(*label);
                        let group_class = if is_open { "nav-group open" } else { "nav-group" };
                        let parent_class = if has { "nav-item nav-parent" } else { "nav-item nav-parent disabled" };
                        rsx! {
                            div { class: "{group_class}",
                                div {
                                    class: "{parent_class}",
                                    onclick: move |_| {
                                        if has {
                                            let mut e = expanded.write();
                                            if !e.remove(*label) { e.insert(label.to_string()); }
                                        }
                                    },
                                    {icon(icon_id, "1.7")}
                                    span { class: "label", "{label}" }
                                    if has {
                                        svg { class: "chev", "viewBox": "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": "2",
                                            dangerous_inner_html: "<use href=\"#i-chev-right\"/>" }
                                    }
                                }
                                if has {
                                    div { class: "nav-sub",
                                        for m in kinds_in {
                                            {
                                                let id = m.id();
                                                let sub_active = id == active_id;
                                                let sub_class = if sub_active { "nav-subitem active" } else { "nav-subitem" };
                                                let label = m.label();
                                                let is_default = default_page() == id;
                                                let (id_click, id_ctx, id_pin) = (id.clone(), id.clone(), id.clone());
                                                let ctx_label = label.clone();
                                                rsx! {
                                                    div {
                                                        class: "{sub_class}",
                                                        onclick: move |_| switch_kind.call(id_click.clone()),
                                                        oncontextmenu: move |e| {
                                                            e.prevent_default();
                                                            let p = e.client_coordinates();
                                                            nav_menu.set(Some(NavMenu { x: p.x, y: p.y, key: id_ctx.clone(), label: ctx_label.clone(), is_default }));
                                                        },
                                                        span { class: "label", "{label}" }
                                                        span { class: "nav-right",
                                                            if is_default {
                                                                span {
                                                                    class: "def-pin tip settling", "data-tip": "Opens on launch",
                                                                    {icon("i-home", "1.9")}
                                                                }
                                                            } else {
                                                                button {
                                                                    class: "set-pin tip", "data-tip": "Set as default page", tabindex: "-1",
                                                                    onclick: move |e| { e.stop_propagation(); set_default.call(id_pin.clone()); },
                                                                    {icon("i-home", "1.7")}
                                                                }
                                                            }
                                                            if sub_active && total > 0 {
                                                                span { class: "count tnum", "{total}" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                }
                div { class: "nav-item nav-about",
                    onclick: move |_| { about_open.set(true); check_update.call(()); },
                    {kompass_mark("")}
                    span { class: "label", "About Kompass" }
                }
            }

            // ===== Main =====
            main {
                class: if detail().is_some() || multi_logs() > 0 {
                    if detail_full() { "main detail-open detail-fullscreen" } else { "main detail-open" }
                } else { "main" },
                style: "--detail-w: {detail_w()}px",
                if multi_logs() > 0 {
                    MultiLogPanel {
                        logs,
                        label: multi_label(),
                        on_close: move |_| { send_cmd(Cmd::StopLogs); multi_logs.set(0); },
                    }
                }
                if let Some(t) = detail() {
                    DetailPanel {
                        target: t,
                        ctx: ctx.clone(),
                        detail_tab,
                        detail_full,
                        manifest,
                        manifest_err,
                        logs,
                        detail_w,
                        resize_start,
                        on_switch: switch_tab,
                        on_close: close_detail,
                        ask_delete,
                        port_forwards: port_forwards(),
                        events: events(),
                        ctrl_pods: ctrl_pods(),
                        on_open: open_detail,
                        on_view_pods: view_pods_for,
                    }
                }
                if overview_on() {
                    section { class: "screen",
                        OverviewScreen { data: overview(), ctx: ctx.clone(), on_refresh: move |_| send_cmd(Cmd::FetchOverview) }
                    }
                } else {
                section { class: "screen",
                    div { class: "list-head",
                        div { class: "list-titlerow",
                            span { class: "list-title", "{title}" }
                            if total > 0 {
                                span { class: "list-count tnum", "{total}" }
                            }
                        }
                        div { class: "toolbar",
                            SearchBox {
                                value: query_val.clone(),
                                placeholder: "Search by name or namespace…",
                                on_change: {
                                    let key = query_key.clone();
                                    move |v: String| {
                                        if v.is_empty() {
                                            queries.write().remove(&key);
                                        } else {
                                            queries.write().insert(key.clone(), v);
                                        }
                                    }
                                },
                            }
                            div { class: "ns-wrap",
                                button {
                                    class: if active_status.is_some() { "filter-chip active" } else { "filter-chip" },
                                    onclick: move |_| status_open.toggle(),
                                    {icon("i-filter", "1.8")}
                                    {active_status.clone().unwrap_or_else(|| "Status".into())}
                                    if active_status.is_some() {
                                        span {
                                            class: "ns-close",
                                            style: "opacity:1",
                                            onclick: move |e| { e.stop_propagation(); status_filter.set(None); },
                                            {icon("i-x", "2.4")}
                                        }
                                    }
                                }
                                if status_open() {
                                    div { class: "menu-scrim", onclick: move |_| status_open.set(false) }
                                    div { class: "menu under",
                                        div { class: "menu-head", "Status" }
                                        div {
                                            class: "menu-item",
                                            onclick: move |_| { status_filter.set(None); status_open.set(false); },
                                            span { "All statuses" }
                                            if active_status.is_none() { span { class: "check", {icon("i-check", "3")} } }
                                        }
                                        div { class: "menu-sep" }
                                        for (s, cls) in statuses.iter().cloned() {
                                            {
                                                let is_sel = active_status.as_deref() == Some(s.as_str());
                                                let pick = s.clone();
                                                rsx! {
                                                    div {
                                                        class: "menu-item",
                                                        onclick: move |_| { status_filter.set(Some(pick.clone())); status_open.set(false); },
                                                        span { class: "badge {cls}", span { class: "dot" } "{s}" }
                                                        if is_sel { span { class: "check", {icon("i-check", "3")} } }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { style: "flex: 1" }
                            div { class: "ns-wrap",
                                button {
                                    class: "filter-chip",
                                    onclick: move |_| columns_open.toggle(),
                                    {icon("i-config", "1.8")}
                                    "Columns"
                                }
                                if columns_open() {
                                    div { class: "menu-scrim", onclick: move |_| columns_open.set(false) }
                                    div { class: "menu under", style: "left:auto; right:0",
                                        div { class: "menu-head", "Columns" }
                                        for (key, label) in col_opts.iter().cloned() {
                                            {
                                                let visible = !hidden_for_dialog.contains(&key);
                                                let k = key.clone();
                                                rsx! {
                                                    div {
                                                        class: "menu-item",
                                                        onclick: move |_| toggle_col.call(k.clone()),
                                                        span { "{label}" }
                                                        if visible {
                                                            span { class: "check", {icon("i-check", "3")} }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if cold {
                        SkeletonTable {}
                    } else if empty {
                        div { class: "state-pane",
                            svg {
                                class: "state-icon",
                                "viewBox": "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                "stroke-width": "1.5",
                                dangerous_inner_html: "<use href=\"#i-empty\"/>",
                            }
                            div { class: "state-title", "No {title.to_lowercase()} found" }
                            div { class: "state-body",
                                "No {title.to_lowercase()} in the selected namespace, or none match your search."
                            }
                        }
                    } else {
                        if errored {
                            div { class: "stale-banner",
                                span { class: "badge-dot" }
                                span {
                                    b { "Connection lost. " }
                                    "Showing last known data — Kompass will reconnect automatically."
                                }
                                div { class: "ml-auto",
                                    button {
                                        class: "btn",
                                        onclick: {
                                            let id = active_id.clone();
                                            move |_| send_cmd(Cmd::SetKind(id.clone()))
                                        },
                                        {icon("i-refresh", "1.8")}
                                        "Retry now"
                                    }
                                }
                            }
                        }
                        div {
                            class: "{wrap_class}",
                            onscroll: move |e| {
                                let m = e.data();
                                scroll_top.set(m.scroll_top() as f64);
                                viewport_h.set(m.client_height() as f64);
                            },
                            div { class: "revalidate-bar" }
                            table { class: "table",
                                thead {
                                    tr {
                                        th { class: "col-check", style: "width:48px" }
                                        th { class: "sortable", style: "width:260px", onclick: move |_| sort_click.call(SortKey::Name),
                                            "Name" span { class: "sort-ind", "{sort_ind(SortKey::Name)}" }
                                        }
                                        if vis.ns {
                                            th { class: "sortable", style: "width:200px", onclick: move |_| sort_click.call(SortKey::Namespace),
                                                "Namespace" span { class: "sort-ind", "{sort_ind(SortKey::Namespace)}" }
                                            }
                                        }
                                        if vis.status {
                                            th { class: "sortable", style: "width:140px", onclick: move |_| sort_click.call(SortKey::Status),
                                                "Status" span { class: "sort-ind", "{sort_ind(SortKey::Status)}" }
                                            }
                                        }
                                        for (i, col) in kind_cols.iter().copied().enumerate() {
                                            if vis.cols.get(i).copied().unwrap_or(true) {
                                                th { class: "sortable", style: "width:120px", onclick: move |_| sort_click.call(SortKey::Col(i)),
                                                    "{col}" span { class: "sort-ind", "{sort_ind(SortKey::Col(i))}" }
                                                }
                                            }
                                        }
                                        if vis.cpu {
                                            th { class: "sortable", style: "width:120px", onclick: move |_| sort_click.call(SortKey::Cpu),
                                                "CPU" span { class: "sort-ind", "{sort_ind(SortKey::Cpu)}" }
                                            }
                                        }
                                        if vis.mem {
                                            th { class: "sortable", style: "width:120px", onclick: move |_| sort_click.call(SortKey::Mem),
                                                "Mem" span { class: "sort-ind", "{sort_ind(SortKey::Mem)}" }
                                            }
                                        }
                                        if vis.age {
                                            th { class: "sortable", style: "width:84px", onclick: move |_| sort_click.call(SortKey::Age),
                                                "Age" span { class: "sort-ind", "{sort_ind(SortKey::Age)}" }
                                            }
                                        }
                                        th { class: "col-actions", style: "width:128px" }
                                    }
                                }
                                tbody {
                                    if top_pad > 0.0 {
                                        tr { td { colspan: "{col_span}", style: "height: {top_pad}px; padding: 0; border: none" } }
                                    }
                                    for r in view[vis_start..vis_end].iter().cloned() {
                                        {
                                            let metric_cells: Vec<(Vec<i64>, String, String)> = if show_metrics {
                                                let key = format!("{}/{}", r.namespace, r.name);
                                                let (ch, mh) = metric_hist.get(&key).cloned().unwrap_or_default();
                                                match metric_map.get(&key) {
                                                    Some((c, m)) => vec![
                                                        (ch, fmt_cpu(*c), "spark".into()),
                                                        (mh, fmt_mem(*m), "spark mem".into()),
                                                    ],
                                                    None => vec![
                                                        (vec![], "–".into(), "spark".into()),
                                                        (vec![], "–".into(), "spark mem".into()),
                                                    ],
                                                }
                                            } else {
                                                Vec::new()
                                            };
                                            rsx! {
                                                ResRow {
                                                    key: "{r.namespace}/{r.name}",
                                                    kind_id: active_id.clone(),
                                                    kind_name: kind_name.clone(),
                                                    on_open: open_detail,
                                                    active: detail().is_some_and(|t| {
                                                        t.namespace == r.namespace && t.name == r.name
                                                    }),
                                                    selected,
                                                    ctx_menu,
                                                    metric_cells,
                                                    vis: vis.clone(),
                                                    row: r,
                                                }
                                            }
                                        }
                                    }
                                    if bot_pad > 0.0 {
                                        tr { td { colspan: "{col_span}", style: "height: {bot_pad}px; padding: 0; border: none" } }
                                    }
                                }
                            }
                        }
                    }
                }
                }
            }

            // ===== Bulk action bar =====
            div { class: if sel_count > 0 { "bulkbar show" } else { "bulkbar" },
                span { class: "n", "{sel_count} selected" }
                span { class: "sep" }
                button {
                    class: "btn btn-ghost",
                    onclick: {
                        let aid = active_id.clone();
                        move |_| {
                            for key in selected().iter() {
                                if let Some((ns, name)) = key.split_once('/') {
                                    send_cmd(Cmd::Restart { kind_id: aid.clone(), namespace: ns.to_string(), name: name.to_string() });
                                }
                            }
                            selected.write().clear();
                        }
                    },
                    {icon("i-restart", "1.7")}
                    "Restart"
                }
                button {
                    class: "btn btn-ghost",
                    onclick: {
                        let kn_logs = kind_name.clone();
                        move |_| {
                        let pods: Vec<(String, String)> = selected()
                            .iter()
                            .filter_map(|k| k.split_once('/').map(|(a, b)| (a.to_string(), b.to_string())))
                            .collect();
                        if !pods.is_empty() {
                            logs.write().clear();
                            detail.set(None);
                            let n = pods.len();
                            let noun = kn_logs.to_lowercase();
                            let label = if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") };
                            send_cmd(Cmd::StartLogsPods(pods.clone()));
                            multi_logs.set(n);
                            multi_label.set(label);
                        }
                        selected.write().clear();
                    }
                    },
                    {icon("i-logs", "1.7")}
                    "Logs"
                }
                button {
                    class: "btn btn-ghost btn-danger",
                    onclick: {
                        let aid = active_id.clone();
                        let kn = kind_name.clone();
                        move |_| {
                            let cmds: Vec<Cmd> = selected().iter().filter_map(|key| {
                                key.split_once('/').map(|(ns, name)| Cmd::Delete {
                                    kind_id: aid.clone(),
                                    namespace: ns.to_string(),
                                    name: name.to_string(),
                                    force: false,
                                })
                            }).collect();
                            let n = cmds.len();
                            ask_delete.call(DeleteReq {
                                is_pod: kn == "Pod",
                                message: format!("Delete {n} selected {kn}?"),
                                cmds,
                                close_detail: false,
                            });
                            selected.write().clear();
                        }
                    },
                    {icon("i-trash", "1.7")}
                    "Delete"
                }
                span { class: "sep" }
                button {
                    class: "icon-btn",
                    onclick: move |_| selected.write().clear(),
                    {icon("i-x", "1.8")}
                }
            }

            // ===== Right-click context menu =====
            if let Some(m) = ctx_menu() {
                PodContextMenu { menu: m, ctx_menu, selected, on_open: open_detail, ask_delete }
            }

            // ===== Right-click nav menu (set/clear default boot page) =====
            if let Some(m) = nav_menu() {
                {
                    let pos = format!("left:{}px; top:{}px", m.x, m.y);
                    let key = m.key.clone();
                    rsx! {
                        div {
                            class: "menu-scrim",
                            onclick: move |_| nav_menu.set(None),
                            oncontextmenu: move |e| { e.prevent_default(); nav_menu.set(None); },
                        }
                        div {
                            class: "menu ctx-menu",
                            style: "{pos}",
                            onmounted: move |_| {
                                dioxus::document::eval(
                                    "const m=document.querySelectorAll('.ctx-menu');const el=m[m.length-1];if(el){const r=el.getBoundingClientRect();\
                                     if(r.right>window.innerWidth-8)el.style.left=Math.max(8,window.innerWidth-r.width-8)+'px';\
                                     if(r.bottom>window.innerHeight-8)el.style.top=Math.max(8,window.innerHeight-r.height-8)+'px';}",
                                );
                            },
                            div { class: "menu-head", "{m.label}" }
                            div {
                                class: if m.is_default { "menu-item is-set" } else { "menu-item" },
                                onclick: move |_| { set_default.call(key.clone()); nav_menu.set(None); },
                                {icon("i-pin", "1.7")}
                                if m.is_default { "Clear default page" } else { "Set as default page" }
                            }
                        }
                    }
                }
            }

            // ===== Resize drag-capture overlay =====
            if resize_start().is_some() {
                ResizeCapture { detail_w, resize_start }
            }

            // ===== ⌘K command palette =====
            if palette_open() {
                div { class: "scrim show", onclick: move |_| palette_open.set(false) }
                div { class: "palette show",
                    div { class: "pal-input",
                        {icon("i-search", "1.8")}
                        input {
                            autofocus: true,
                            autocomplete: "off",
                            autocapitalize: "off",
                            spellcheck: "false",
                            "autocorrect": "off",
                            placeholder: "Search kinds, clusters, resources…",
                            value: "{palette_query}",
                            onmounted: move |e| { let _ = e.set_focus(true); },
                            oninput: move |e| { palette_query.set(e.value()); palette_sel.set(0); },
                            onkeydown: move |e| {
                                match e.key() {
                                    Key::ArrowDown => {
                                        e.prevent_default();
                                        if pal_count > 0 { palette_sel.set((pal_sel + 1).min(pal_count - 1)); }
                                    }
                                    Key::ArrowUp => {
                                        e.prevent_default();
                                        palette_sel.set(pal_sel.saturating_sub(1));
                                    }
                                    Key::Enter => {
                                        if let Some(it) = pal_for_enter.get(pal_sel) {
                                            dispatch.call(it.action.clone());
                                        }
                                    }
                                    Key::Escape => palette_open.set(false),
                                    _ => {}
                                }
                            },
                        }
                        span { class: "ctx-pill",
                            span { class: "d" }
                            "{ctx}"
                        }
                    }
                    div { class: "pal-results",
                        if pal_count == 0 {
                            div { class: "pal-empty", "No results" }
                        }
                        for (i, it) in pal_items.iter().enumerate() {
                            {
                                let show_label = i == 0 || pal_items[i - 1].group != it.group;
                                let sel = i == pal_sel;
                                let action = it.action.clone();
                                rsx! {
                                    if show_label {
                                        div { class: "pal-group-label", "{it.group}" }
                                    }
                                    div {
                                        class: if sel { "pal-item sel" } else { "pal-item" },
                                        onmouseenter: move |_| palette_sel.set(i),
                                        onclick: move |_| dispatch.call(action.clone()),
                                        div { class: "pi-icon", {icon(it.icon, "1.7")} }
                                        div { class: "pi-main",
                                            div { class: "pi-title", "{it.title}" }
                                            if !it.sub.is_empty() {
                                                div { class: "pi-sub", "{it.sub}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "pal-foot",
                        span { class: "hint", span { class: "kbd", "↑↓" } "navigate" }
                        span { class: "hint", span { class: "kbd", "↵" } "select" }
                        span { class: "hint", span { class: "kbd", "esc" } "close" }
                    }
                }
            }

            // ===== Delete confirmation (non-pod kinds) =====
            if let Some(req) = confirm() {
                div { class: "scrim show", onclick: move |_| confirm.set(None) }
                div { class: "confirm-dialog",
                    div { class: "cd-icon", {icon("i-warning", "1.8")} }
                    div { class: "cd-title", "{req.message}" }
                    div { class: "cd-body", "This can't be undone." }
                    div { class: "cd-actions",
                        button { class: "btn", onclick: move |_| confirm.set(None), "Cancel" }
                        button {
                            class: "btn btn-danger-solid",
                            onclick: move |_| { run_delete.call(req.clone()); confirm.set(None); },
                            {icon("i-trash", "1.7")}
                            "Delete"
                        }
                    }
                }
            }

            // ===== About popup =====
            if about_open() {
                div { class: "scrim show", onclick: move |_| about_open.set(false) }
                div { class: "about-dialog",
                    {kompass_mark("about-mark")}
                    div { class: "about-name", "Kompass" }
                    div { class: "about-ver", "Version " {env!("CARGO_PKG_VERSION")} }
                    div { class: "about-upd",
                        if checking_update() {
                            "Checking for updates…"
                        } else if let Some((v, _)) = update_info() {
                            button {
                                class: "about-upd-link",
                                onclick: move |_| open_url("https://github.com/erango/kompass/releases/latest"),
                                "Update available: {v} →"
                            }
                        } else if update_checked() {
                            "You're on the latest version."
                        }
                    }
                    div { class: "about-love",
                        "Made with "
                        span { class: "heart", "♥" }
                        " by "
                        b { "@erango" }
                    }
                    div {
                        class: "about-repo",
                        onclick: move |_| open_url("https://github.com/erango/kompass"),
                        {icon("i-arrow-right", "1.8")}
                        "github.com/erango/kompass"
                    }
                }
            }

            // ===== Update banner =====
            if let Some((ver, via_brew)) = update_info() {
                if !update_dismissed() {
                    div { class: "update-banner",
                        span { class: "ub-mark", {kompass_mark("")} }
                        span { class: "ub-text", "Kompass " b { "{ver}" } " is available." }
                        if via_brew {
                            CopyButton {
                                text: "brew upgrade --cask kompass".to_string(),
                                class: "ub-btn tip".to_string(),
                                tip: "Copy to clipboard".to_string(),
                                label: "brew upgrade".to_string(),
                            }
                        } else {
                            button {
                                class: "ub-btn",
                                onclick: move |_| open_url("https://github.com/erango/kompass/releases/latest"),
                                {icon("i-dl", "1.7")}
                                "Download"
                            }
                        }
                        button {
                            class: "ub-btn ghost",
                            onclick: move |_| open_url("https://github.com/erango/kompass/releases/latest"),
                            "Notes"
                        }
                        button {
                            class: "ub-x", onclick: move |_| update_dismissed.set(true),
                            {icon("i-x", "1.8")}
                        }
                    }
                }
            }

            // ===== Toasts =====
            div { class: "toasts",
                for t in toasts() {
                    div {
                        class: if t.ok { "toast ok" } else { "toast err" },
                        key: "{t.id}",
                        onclick: move |_| toasts.write().retain(|x| x.id != t.id),
                        span { class: "ic",
                            if t.ok { {icon("i-check", "2.2")} } else { {icon("i-warning", "1.8")} }
                        }
                        span { class: "msg", "{t.msg}" }
                    }
                }
            }
        }
    }
}

/// Actions menu for a pod, opened on right-click and anchored at the cursor.
#[component]
fn PodContextMenu(
    menu: CtxMenu,
    ctx_menu: Signal<Option<CtxMenu>>,
    selected: Signal<BTreeSet<String>>,
    on_open: EventHandler<OpenReq>,
    ask_delete: EventHandler<DeleteReq>,
) -> Element {
    let key = format!("{}/{}", menu.target.namespace, menu.target.name);
    let pos = format!("left:{}px; top:{}px", menu.x, menu.y);
    let close = move |_| ctx_menu.set(None);

    let t_logs = menu.target.clone();
    let t_exec = menu.target.clone();
    let t_yaml = menu.target.clone();
    let t_events = menu.target.clone();
    let t_restart = menu.target.clone();
    let t_delete = menu.target.clone();
    let t_force = menu.target.clone();
    let is_node = menu.target.kind_name == "Node";
    let (n_cordon, n_uncordon, n_drain) =
        (menu.target.name.clone(), menu.target.name.clone(), menu.target.name.clone());
    rsx! {
        div {
            class: "menu-scrim",
            onclick: close,
            oncontextmenu: move |e| { e.prevent_default(); ctx_menu.set(None); },
        }
        div {
            class: "menu ctx-menu",
            style: "{pos}",
            onmounted: move |_| {
                dioxus::document::eval(
                    "const m=document.querySelector('.ctx-menu'); if(m){const r=m.getBoundingClientRect();\
                     if(r.right>window.innerWidth-8)m.style.left=Math.max(8,window.innerWidth-r.width-8)+'px';\
                     if(r.bottom>window.innerHeight-8)m.style.top=Math.max(8,window.innerHeight-r.height-8)+'px';}",
                );
            },
            div { class: "menu-head", "{menu.target.name}" }
            if has_logs(&menu.target.kind_name) {
                div {
                    class: "menu-item",
                    onclick: move |_| { on_open.call(OpenReq { target: t_logs.clone(), tab: DetailTab::Logs }); ctx_menu.set(None); },
                    {icon("i-logs", "1.7")}
                    "View logs"
                }
                div {
                    class: "menu-item",
                    onclick: move |_| { on_open.call(OpenReq { target: t_exec.clone(), tab: DetailTab::Exec }); ctx_menu.set(None); },
                    {icon("i-terminal", "1.7")}
                    "Exec / shell"
                }
            }
            div {
                class: "menu-item",
                onclick: move |_| { on_open.call(OpenReq { target: t_yaml.clone(), tab: DetailTab::Yaml }); ctx_menu.set(None); },
                {icon("i-yaml", "1.7")}
                "Edit YAML"
            }
            div {
                class: "menu-item",
                onclick: move |_| { on_open.call(OpenReq { target: t_events.clone(), tab: DetailTab::Events }); ctx_menu.set(None); },
                {icon("i-events", "1.7")}
                "View events"
            }
            if is_node {
                div {
                    class: "menu-item",
                    onclick: move |_| { send_cmd(Cmd::Cordon { name: n_cordon.clone(), on: true }); ctx_menu.set(None); },
                    {icon("i-lock", "1.7")}
                    "Cordon"
                }
                div {
                    class: "menu-item",
                    onclick: move |_| { send_cmd(Cmd::Cordon { name: n_uncordon.clone(), on: false }); ctx_menu.set(None); },
                    {icon("i-check", "2.2")}
                    "Uncordon"
                }
                div {
                    class: "menu-item danger",
                    onclick: move |_| { send_cmd(Cmd::Drain { name: n_drain.clone() }); ctx_menu.set(None); },
                    {icon("i-dl", "1.7")}
                    "Drain"
                }
            }
            if is_workload(&menu.target.kind_name) {
                div {
                    class: "menu-item",
                    onclick: move |_| {
                        send_cmd(Cmd::Restart { kind_id: t_restart.kind_id.clone(), namespace: t_restart.namespace.clone(), name: t_restart.name.clone() });
                        ctx_menu.set(None);
                    },
                    {icon("i-restart", "1.7")}
                    "Restart"
                }
            }
            div {
                class: "menu-item",
                onclick: move |_| {
                    let mut s = selected.write();
                    if !s.remove(&key) { s.insert(key.clone()); }
                    ctx_menu.set(None);
                },
                {icon("i-check", "2.4")}
                "Select"
            }
            div { class: "menu-sep" }
            div {
                class: "menu-item danger",
                onclick: move |_| {
                    ask_delete.call(DeleteReq {
                        is_pod: t_delete.kind_name == "Pod",
                        message: format!("Delete {} {}?", t_delete.kind_name, t_delete.name),
                        cmds: vec![Cmd::Delete { kind_id: t_delete.kind_id.clone(), namespace: t_delete.namespace.clone(), name: t_delete.name.clone(), force: false }],
                        close_detail: false,
                    });
                    ctx_menu.set(None);
                },
                {icon("i-trash", "1.7")}
                "Delete"
            }
            div {
                class: "menu-item danger",
                onclick: move |_| {
                    // Force delete is destructive (skips graceful shutdown) — always confirm.
                    ask_delete.call(DeleteReq {
                        is_pod: false,
                        message: format!("Force delete {} {}?", t_force.kind_name, t_force.name),
                        cmds: vec![Cmd::Delete { kind_id: t_force.kind_id.clone(), namespace: t_force.namespace.clone(), name: t_force.name.clone(), force: true }],
                        close_detail: false,
                    });
                    ctx_menu.set(None);
                },
                {icon("i-trash", "1.7")}
                "Force delete"
            }
        }
    }
}

/// Full-screen overlay that captures mouse-move while resizing the detail panel.
#[component]
fn ResizeCapture(detail_w: Signal<f64>, resize_start: Signal<Option<(f64, f64)>>) -> Element {
    rsx! {
        div {
            class: "resize-capture",
            onmousemove: move |e| {
                if let Some((start_x, start_w)) = resize_start() {
                    // Dragging the left edge leftwards widens the panel.
                    let dx = e.client_coordinates().x - start_x;
                    let w = (start_w - dx).clamp(380.0, 1100.0);
                    detail_w.set(w);
                }
            },
            onmouseup: move |_| resize_start.set(None),
        }
    }
}

#[component]
fn ResRow(
    kind_id: String,
    kind_name: String,
    row: ResourceRow,
    selected: Signal<BTreeSet<String>>,
    ctx_menu: Signal<Option<CtxMenu>>,
    on_open: EventHandler<OpenReq>,
    active: bool,
    metric_cells: Vec<(Vec<i64>, String, String)>,
    vis: ColVis,
) -> Element {
    let key = format!("{}/{}", row.namespace, row.name);
    let is_sel = selected().contains(&key);
    let is_pod = kind_name == "Pod";
    let target = DetailTarget {
        kind_id,
        kind_name,
        namespace: row.namespace.clone(),
        name: row.name.clone(),
        status: row.status.clone(),
        status_class: row.status_class.clone(),
        cols: row.cols.clone(),
        age: row.age.clone(),
    };

    let badge = format!("badge {}", row.status_class);
    let row_class = if active {
        "active-row"
    } else if is_sel {
        "selected"
    } else {
        ""
    };
    let check_class = if is_sel { "row-check checked" } else { "row-check" };

    let key_toggle = key.clone();
    let open_target = target.clone();
    let menu_target = target.clone();
    rsx! {
        tr {
            class: "{row_class}",
            onclick: move |_| on_open.call(OpenReq { target: open_target.clone(), tab: DetailTab::Summary }),
            oncontextmenu: move |e| {
                e.prevent_default();
                let p = e.client_coordinates();
                ctx_menu.set(Some(CtxMenu {
                    x: p.x,
                    y: p.y,
                    target: menu_target.clone(),
                }));
            },
            td { class: "col-check",
                div {
                    class: "{check_class}",
                    onclick: move |e| {
                        e.stop_propagation();
                        let mut s = selected.write();
                        if !s.remove(&key_toggle) {
                            s.insert(key_toggle.clone());
                        }
                    },
                    if is_sel { {icon("i-check", "3")} }
                }
            }
            td { class: "col-name",
                div { class: "cell-name tip", "data-tip": "{row.name}",
                    span { class: "nm", "{row.name}" }
                }
            }
            if vis.ns {
                td { class: "cell-ns", "{row.namespace}" }
            }
            if vis.status {
                td {
                    span { class: "{badge}",
                        span { class: "dot" }
                        "{row.status}"
                    }
                }
            }
            for (i, col) in row.cols.iter().enumerate() {
                if vis.cols.get(i).copied().unwrap_or(true) {
                    td { class: if is_pod && i == 0 { "col-ctr" } else { "" },
                        if is_pod && i == 0 {
                            ContainerSquares { containers: row.containers.clone(), large: false }
                        } else {
                            span { class: "ready", "{col}" }
                        }
                    }
                }
            }
            for (mi, (series, label, cls)) in metric_cells.iter().enumerate() {
                if (mi == 0 && vis.cpu) || (mi == 1 && vis.mem) {
                td {
                    div { class: "metric-cell",
                        if series.len() >= 2 {
                            svg {
                                class: "{cls}",
                                "viewBox": "0 0 56 18",
                                "preserveAspectRatio": "none",
                                polyline {
                                    points: sparkline_points(series),
                                    fill: "none",
                                    stroke: "currentColor",
                                    "stroke-width": "1.5",
                                    "stroke-linejoin": "round",
                                    "stroke-linecap": "round",
                                }
                            }
                        }
                        span { class: "metric-val", "{label}" }
                    }
                }
                }
            }
            if vis.age {
                td { class: "cell-age", "{row.age}" }
            }
            td { class: "col-actions",
                div { class: "row-actions",
                    button {
                        class: "icon-btn",
                        onclick: {
                            let t = target.clone();
                            move |e: MouseEvent| {
                                e.stop_propagation();
                                on_open.call(OpenReq { target: t.clone(), tab: DetailTab::Yaml });
                            }
                        },
                        {icon("i-yaml", "1.7")}
                    }
                    if has_logs(&target.kind_name) {
                        button {
                            class: "icon-btn",
                            onclick: {
                                let t = target.clone();
                                move |e: MouseEvent| {
                                    e.stop_propagation();
                                    on_open.call(OpenReq { target: t.clone(), tab: DetailTab::Logs });
                                }
                            },
                            {icon("i-logs", "1.7")}
                        }
                    }
                    button {
                        class: "icon-btn",
                        onclick: {
                            let t = target.clone();
                            move |e: MouseEvent| {
                                e.stop_propagation();
                                let p = e.client_coordinates();
                                ctx_menu.set(Some(CtxMenu { x: p.x, y: p.y, target: t.clone() }));
                            }
                        },
                        {icon("i-more", "1.8")}
                    }
                }
            }
        }
    }
}

fn badge_class(status: &str) -> &'static str {
    match status {
        "Running" => "badge running",
        "Pending" => "badge pending",
        "Failed" => "badge failed",
        "Succeeded" => "badge neutral",
        _ => "badge neutral",
    }
}

/// Parsed summary fields pulled from a pod manifest.
struct PodSummary {
    node: Option<String>,
    pod_ip: Option<String>,
    labels: Vec<(String, String)>,
    containers: Vec<(String, String)>,
}

fn parse_summary(yaml: &str) -> Option<PodSummary> {
    let v: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let node = v["spec"]["nodeName"].as_str().map(str::to_string);
    let pod_ip = v["status"]["podIP"].as_str().map(str::to_string);
    let labels = v["metadata"]["labels"]
        .as_mapping()
        .map(|m| {
            m.iter()
                .filter_map(|(k, val)| {
                    Some((k.as_str()?.to_string(), val.as_str().unwrap_or("").to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let containers = v["spec"]["containers"]
        .as_sequence()
        .map(|s| {
            s.iter()
                .filter_map(|c| {
                    Some((
                        c["name"].as_str()?.to_string(),
                        c["image"].as_str().unwrap_or("").to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(PodSummary { node, pod_ip, labels, containers })
}

/// Tokenize one YAML line into `(css-class, text)` spans for syntax coloring.
/// Heuristic, not a full parser — keys, the `key:` colon, quoted strings,
/// numbers, booleans/null, and comments. Empty class = default foreground.
fn highlight_yaml(line: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let indent = line.len() - line.trim_start().len();
    if indent > 0 {
        out.push(("", line[..indent].to_string()));
    }
    let mut rest = &line[indent..];
    if rest.is_empty() {
        return out;
    }
    // Whole-line comment.
    if rest.starts_with('#') {
        out.push(("tok-comment", rest.to_string()));
        return out;
    }
    // Leading list dashes.
    while rest.starts_with("- ") || rest == "-" {
        if rest == "-" {
            out.push(("tok-punct", "-".to_string()));
            return out;
        }
        out.push(("tok-punct", "- ".to_string()));
        rest = &rest[2..];
    }
    // `key:` — first colon followed by space/EOL, with no space in the key.
    let key_colon = rest.as_bytes().iter().enumerate().find_map(|(i, &b)| {
        if b == b':' && (i + 1 == rest.len() || rest.as_bytes()[i + 1] == b' ') && !rest[..i].contains(' ')
        {
            Some(i)
        } else {
            None
        }
    });
    match key_colon {
        Some(ci) => {
            out.push(("tok-key", rest[..ci].to_string()));
            out.push(("tok-punct", ":".to_string()));
            highlight_yaml_value(&rest[ci + 1..], &mut out);
        }
        None => highlight_yaml_value(rest, &mut out),
    }
    out
}

fn highlight_yaml_value(after: &str, out: &mut Vec<(&'static str, String)>) {
    let lead = after.len() - after.trim_start().len();
    if lead > 0 {
        out.push(("", after[..lead].to_string()));
    }
    let val = after.trim_start();
    if val.is_empty() {
        return;
    }
    // Split off a trailing inline comment.
    let (val, comment) = match val.find(" #") {
        Some(ci) => (&val[..ci], Some(&val[ci..])),
        None => (val, None),
    };
    if !val.is_empty() {
        let quoted = (val.starts_with('"') && val.ends_with('"') && val.len() > 1)
            || (val.starts_with('\'') && val.ends_with('\'') && val.len() > 1);
        let is_bool = matches!(
            val,
            "true" | "false" | "null" | "~" | "True" | "False" | "Null" | "yes" | "no"
        );
        let class = if quoted {
            "tok-str"
        } else if is_bool {
            "tok-bool"
        } else if val.parse::<f64>().is_ok() {
            "tok-num"
        } else {
            ""
        };
        out.push((class, val.to_string()));
    }
    if let Some(c) = comment {
        out.push(("tok-comment", c.to_string()));
    }
}

/// A log line's timestamp (HH:MM:SS) and message, split from the RFC3339 prefix.
fn split_log(line: &str) -> (String, &str) {
    match line.split_once(' ') {
        Some((ts, msg)) => (ts.get(11..19).unwrap_or("").to_string(), msg),
        None => (String::new(), line),
    }
}

/// Resizable detail side panel with Summary / YAML / Logs tabs + fullscreen.
#[component]
fn DetailPanel(
    target: DetailTarget,
    ctx: String,
    detail_tab: Signal<DetailTab>,
    detail_full: Signal<bool>,
    manifest: Signal<Option<String>>,
    manifest_err: Signal<Option<String>>,
    logs: Signal<Vec<LogEntry>>,
    detail_w: Signal<f64>,
    resize_start: Signal<Option<(f64, f64)>>,
    on_switch: EventHandler<DetailTab>,
    on_close: EventHandler<()>,
    ask_delete: EventHandler<DeleteReq>,
    port_forwards: Vec<PortForward>,
    events: Vec<EventRow>,
    ctrl_pods: Vec<ResourceRow>,
    on_open: EventHandler<OpenReq>,
    on_view_pods: EventHandler<String>,
) -> Element {
    let mut cls = String::from("detail open enter");
    if detail_full() {
        cls.push_str(" fullscreen");
    }
    if resize_start().is_some() {
        cls.push_str(" resizing");
    }
    let tab = detail_tab();
    let tab_cls = |t: DetailTab| if tab == t { "detail-tab active" } else { "detail-tab" };
    let mut scale_val = use_signal(String::new);
    // Prefill (and reset after apply) with the manifest's current replica count.
    use_effect(move || {
        if let Some(c) = manifest()
            .as_deref()
            .and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
            .and_then(|v| v["spec"]["replicas"].as_i64())
        {
            scale_val.set(c.to_string());
        }
    });
    let a_ns = target.namespace.clone();
    let a_name = target.name.clone();
    let a_kind_id = target.kind_id.clone();
    let a_kind_name = target.kind_name.clone();
    let can_workload = is_workload(&a_kind_name);
    // Current replica count (from the manifest) → disable Scale when unchanged.
    let cur_replicas: Option<i64> = manifest()
        .as_deref()
        .and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
        .and_then(|v| v["spec"]["replicas"].as_i64());
    let scale_disabled = {
        let s = scale_val();
        let s = s.trim();
        s.is_empty() || s.parse::<i64>().ok() == cur_replicas
    };
    let scale_tip = if scale_disabled {
        "Set a new replica count".to_string()
    } else {
        format!("Scale to {} replicas", scale_val().trim())
    };

    rsx! {
        div { class: "{cls}",
            // resize handle (drag the left edge)
            div {
                class: "detail-resize",
                onmousedown: move |e| {
                    resize_start.set(Some((e.client_coordinates().x, detail_w())));
                },
            }

            // ===== Header =====
            div { class: "detail-head",
                div { class: "detail-toprow",
                    div { class: "detail-kind",
                        "{target.kind_name}"
                        span { class: "cluster-tag",
                            span { class: "d" }
                            "{ctx}"
                        }
                    }
                    div { class: "detail-headctrls",
                        button {
                            class: "icon-btn tip",
                            "data-tip": "Fullscreen",
                            onclick: move |_| detail_full.toggle(),
                            {icon("i-fullscreen", "1.7")}
                        }
                        button {
                            class: "icon-btn tip",
                            "data-tip": "Close",
                            onclick: move |_| on_close.call(()),
                            {icon("i-x", "1.8")}
                        }
                    }
                }
                div { class: "detail-title", "{target.name}" }
                div { class: "detail-headmeta",
                    span { class: badge_class(&target.status),
                        span { class: "dot" }
                        "{target.status}"
                    }
                    if let Some(first) = target.cols.first() {
                        span { class: "ready", "{first}" }
                    }
                    span { class: "cell-ns", "{target.namespace}" }
                    span { class: "cell-age", "{target.age}" }
                }
            }

            // ===== Action toolbar =====
            div { class: "detail-actions",
                if a_kind_name == "Node" {
                    {
                        let unsched = manifest()
                            .as_deref()
                            .and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
                            .map(|v| v["spec"]["unschedulable"].as_bool().unwrap_or(false))
                            .unwrap_or(false);
                        let nm = a_name.clone();
                        let nm2 = a_name.clone();
                        rsx! {
                            if unsched {
                                button {
                                    class: "btn",
                                    onclick: move |_| send_cmd(Cmd::Cordon { name: nm.clone(), on: false }),
                                    {icon("i-check", "2.2")} "Uncordon"
                                }
                            } else {
                                button {
                                    class: "btn",
                                    onclick: move |_| send_cmd(Cmd::Cordon { name: nm.clone(), on: true }),
                                    {icon("i-lock", "1.7")} "Cordon"
                                }
                            }
                            button {
                                class: "btn btn-danger",
                                onclick: move |_| send_cmd(Cmd::Drain { name: nm2.clone() }),
                                {icon("i-dl", "1.7")} "Drain"
                            }
                        }
                    }
                }
                if can_workload {
                    button {
                        class: "btn",
                        onclick: {
                            let (ns, nm, id) = (a_ns.clone(), a_name.clone(), a_kind_id.clone());
                            move |_| send_cmd(Cmd::Restart { kind_id: id.clone(), namespace: ns.clone(), name: nm.clone() })
                        },
                        {icon("i-restart", "1.7")}
                        "Restart"
                    }
                    div { class: "scale-group",
                        span { class: "scale-label", "replicas" }
                        input {
                            class: "scale-input tip",
                            r#type: "number",
                            min: "0",
                            "data-tip": "Desired replica count",
                            value: "{scale_val}",
                            oninput: move |e| scale_val.set(e.value()),
                        }
                        button {
                            class: "scale-go tip",
                            disabled: scale_disabled,
                            "data-tip": "{scale_tip}",
                            onclick: {
                                let (ns, nm, id) = (a_ns.clone(), a_name.clone(), a_kind_id.clone());
                                move |_| {
                                    if let Ok(replicas) = scale_val().trim().parse::<i32>() {
                                        send_cmd(Cmd::Scale { kind_id: id.clone(), namespace: ns.clone(), name: nm.clone(), replicas });
                                    }
                                }
                            },
                            {icon("i-check", "2.4")}
                        }
                    }
                }
                button {
                    class: "btn btn-danger",
                    onclick: {
                        let (ns, nm, id, kn) = (a_ns.clone(), a_name.clone(), a_kind_id.clone(), a_kind_name.clone());
                        move |_| {
                            ask_delete.call(DeleteReq {
                                is_pod: kn == "Pod",
                                message: format!("Delete {kn} {nm}?"),
                                cmds: vec![Cmd::Delete { kind_id: id.clone(), namespace: ns.clone(), name: nm.clone(), force: false }],
                                close_detail: true,
                            });
                        }
                    },
                    {icon("i-trash", "1.7")}
                    "Delete"
                }
            }

            // ===== Tabs =====
            div { class: "detail-tabs",
                button { class: tab_cls(DetailTab::Summary), onclick: move |_| on_switch.call(DetailTab::Summary),
                    {icon("i-summary", "1.7")} "Summary"
                }
                if is_data_kind(&a_kind_name) {
                    button { class: tab_cls(DetailTab::Data), onclick: move |_| on_switch.call(DetailTab::Data),
                        {icon("i-config", "1.7")} "Data"
                    }
                }
                button { class: tab_cls(DetailTab::Yaml), onclick: move |_| on_switch.call(DetailTab::Yaml),
                    {icon("i-yaml", "1.7")} "YAML"
                }
                if has_logs(&a_kind_name) {
                    button { class: tab_cls(DetailTab::Logs), onclick: move |_| on_switch.call(DetailTab::Logs),
                        {icon("i-logs", "1.7")} "Logs"
                    }
                }
                if a_kind_name == "Pod" {
                    button { class: tab_cls(DetailTab::Exec), onclick: move |_| on_switch.call(DetailTab::Exec),
                        {icon("i-terminal", "1.7")} "Exec"
                    }
                }
                button { class: tab_cls(DetailTab::Events), onclick: move |_| on_switch.call(DetailTab::Events),
                    {icon("i-events", "1.7")} "Events"
                }
            }

            // ===== Body =====
            div { class: "detail-body",
                match tab {
                    DetailTab::Summary => rsx! { SummaryTab { target: target.clone(), manifest, manifest_err, port_forwards: port_forwards.clone(), ctrl_pods: ctrl_pods.clone(), on_open, on_view_pods } },
                    DetailTab::Data => rsx! { DataTab { key: "{target.namespace}/{target.name}", kind_name: target.kind_name.clone(), manifest, manifest_err } },
                    DetailTab::Yaml => rsx! {
                        YamlTab {
                            key: "{target.namespace}/{target.name}",
                            manifest,
                            manifest_err,
                            kind_id: target.kind_id.clone(),
                            namespace: target.namespace.clone(),
                            name: target.name.clone(),
                        }
                    },
                    DetailTab::Logs => {
                        // Selectable containers (init first, then main) — Pod uses
                        // spec.*, controllers use spec.template.spec.*.
                        let containers: Vec<String> = manifest()
                            .as_deref()
                            .and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
                            .map(|v| {
                                let spec = if target.kind_name == "Pod" {
                                    &v["spec"]
                                } else {
                                    &v["spec"]["template"]["spec"]
                                };
                                let names = |key: &str| {
                                    spec[key]
                                        .as_sequence()
                                        .map(|cs| cs.iter().filter_map(|c| c["name"].as_str().map(String::from)).collect::<Vec<_>>())
                                        .unwrap_or_default()
                                };
                                let mut out = names("initContainers");
                                out.extend(names("containers"));
                                out
                            })
                            .unwrap_or_default();
                        rsx! {
                            LogsTab {
                                key: "{target.namespace}/{target.name}",
                                logs,
                                kind_id: target.kind_id.clone(),
                                namespace: target.namespace.clone(),
                                name: target.name.clone(),
                                containers,
                            }
                        }
                    }
                    DetailTab::Exec => rsx! {
                        ExecTab {
                            key: "{target.namespace}/{target.name}",
                            namespace: target.namespace.clone(),
                            name: target.name.clone(),
                        }
                    },
                    DetailTab::Events => rsx! { EventsTab { events: events.clone() } },
                }
            }
        }
    }
}

#[component]
fn EventsTab(events: Vec<EventRow>) -> Element {
    rsx! {
        if events.is_empty() {
            div { class: "detail-empty",
                {icon("i-events", "1.6")}
                "No events for this object."
            }
        } else {
            div { class: "ev-list",
                for (i, e) in events.iter().enumerate() {
                    div { class: if e.warn { "ev-row warn" } else { "ev-row" }, key: "{i}",
                        span { class: "ev-dot" }
                        div { class: "ev-main",
                            div { class: "ev-head",
                                span { class: "ev-reason", "{e.reason}" }
                                span { class: "ev-age", "{e.age}" }
                            }
                            div { class: "ev-msg", "{e.message}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SummaryTab(
    target: DetailTarget,
    manifest: Signal<Option<String>>,
    manifest_err: Signal<Option<String>>,
    port_forwards: Vec<PortForward>,
    #[props(default)] ctrl_pods: Vec<ResourceRow>,
    #[props(default)] on_open: Option<EventHandler<OpenReq>>,
    #[props(default)] on_view_pods: Option<EventHandler<String>>,
) -> Element {
    let m = manifest();
    let summary = m.as_deref().and_then(parse_summary);
    let cstates = m.as_deref().map(container_states_from_yaml).unwrap_or_default();
    // Container ports (for port-forwarding) + this pod's active forwards.
    let ports: Vec<u16> = m
        .as_deref()
        .and_then(|y| serde_yaml::from_str::<serde_yaml::Value>(y).ok())
        .and_then(|v| {
            v["spec"]["containers"].as_sequence().map(|cs| {
                let mut out = Vec::new();
                for c in cs {
                    if let Some(ps) = c["ports"].as_sequence() {
                        for p in ps {
                            if let Some(n) = p["containerPort"].as_u64() {
                                let port = n as u16;
                                if !out.contains(&port) {
                                    out.push(port);
                                }
                            }
                        }
                    }
                }
                out
            })
        })
        .unwrap_or_default();
    let active: Vec<PortForward> = port_forwards
        .into_iter()
        .filter(|p| p.namespace == target.namespace && p.name == target.name)
        .collect();
    let is_pod = target.kind_name == "Pod";
    rsx! {
        div { class: "sum",
            div { class: "sum-group",
                h4 { "Overview" }
                dl { class: "kv",
                    dt { "Name" }
                    dd { "{target.name}" }
                    dt { "Namespace" }
                    dd { "{target.namespace}" }
                    dt { "Status" }
                    dd { "{target.status}" }
                    for (label, val) in columns_for(&target.kind_name).iter().zip(target.cols.iter()) {
                        dt { "{label}" }
                        dd { class: "mono", "{val}" }
                    }
                    dt { "Age" }
                    dd { "{target.age}" }
                    if let Some(s) = &summary {
                        if let Some(node) = &s.node {
                            dt { "Node" }
                            dd { class: "mono", "{node}" }
                        }
                        if let Some(ip) = &s.pod_ip {
                            dt { "Pod IP" }
                            dd { class: "mono", "{ip}" }
                        }
                    }
                }
            }
            if let Some(s) = &summary {
                if !s.containers.is_empty() {
                    div { class: "sum-group",
                        h4 { "Containers" }
                        for (name, image) in s.containers.clone() {
                            {
                                let cs = cstates.iter().find(|c| c.name == name).cloned();
                                let restarts = cs.as_ref().map(|c| c.restarts).unwrap_or(0);
                                let rs_warn = restarts > 0;
                                rsx! {
                                    div { class: "ctr",
                                        div { class: "ctr-head",
                                            if let Some(c) = cs {
                                                ContainerSquares { containers: vec![c], large: true }
                                            }
                                            span { class: "ctr-name", "{name}" }
                                            div { class: if rs_warn { "ctr-rs warn" } else { "ctr-rs" },
                                                span { class: "lbl", "restarts" }
                                                span { class: "rsv", "{restarts}" }
                                            }
                                        }
                                        span { class: "ctr-img", "{image}" }
                                    }
                                }
                            }
                        }
                    }
                }
                if !s.labels.is_empty() {
                    div { class: "sum-group",
                        h4 { "Labels" }
                        div { class: "chips",
                            for (k, v) in s.labels.clone() {
                                div { class: "chip",
                                    span { class: "k", "{k}=" }
                                    "{v}"
                                }
                            }
                        }
                    }
                }
            } else if manifest_err().is_some() {
                div { class: "detail-empty", "Couldn't load full manifest: {manifest_err().unwrap_or_default()}" }
            } else if m.is_none() {
                div { class: "detail-loading", "Loading manifest…" }
            }
            if is_pod && (!ports.is_empty() || !active.is_empty()) {
                div { class: "sum-group",
                    h4 { "Port forwarding" }
                    for pf in active.iter() {
                        {
                            let lp = pf.local_port;
                            rsx! {
                                div { class: "pf-row",
                                    span { class: "pf-fwd",
                                        span { class: "d" }
                                        "localhost:{pf.local_port} → :{pf.pod_port}"
                                    }
                                    button {
                                        class: "btn btn-ghost btn-danger",
                                        onclick: move |_| send_cmd(Cmd::StopPortForward { local_port: lp }),
                                        "Stop"
                                    }
                                }
                            }
                        }
                    }
                    for port in ports.iter().copied() {
                        if !active.iter().any(|a| a.pod_port == port) {
                            {
                                let (ns, nm) = (target.namespace.clone(), target.name.clone());
                                rsx! {
                                    div { class: "pf-row",
                                        span { class: "pf-port", "container port {port}" }
                                        button {
                                            class: "btn",
                                            onclick: move |_| send_cmd(Cmd::StartPortForward {
                                                namespace: ns.clone(),
                                                name: nm.clone(),
                                                pod_port: port,
                                                local_port: port,
                                            }),
                                            {icon("i-network", "1.7")}
                                            "Forward"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if is_workload(&target.kind_name) {
                div { class: "sum-group",
                    div { class: "sum-pods-head",
                        h4 { "Pods" }
                        if let Some(h) = on_view_pods {
                            {
                                let nm = target.name.clone();
                                rsx! {
                                    button {
                                        class: "btn btn-ghost btn-sm",
                                        onclick: move |_| h.call(nm.clone()),
                                        "View all"
                                        {icon("i-arrow-right", "1.8")}
                                    }
                                }
                            }
                        }
                    }
                    if ctrl_pods.is_empty() {
                        div { class: "detail-empty", "No pods." }
                    } else {
                        div { class: "sp-list",
                            for p in ctrl_pods.iter() {
                                {
                                    let ready = p.cols.first().cloned().unwrap_or_default();
                                    let t = DetailTarget {
                                        kind_id: "pods".into(),
                                        kind_name: "Pod".into(),
                                        namespace: p.namespace.clone(),
                                        name: p.name.clone(),
                                        status: p.status.clone(),
                                        status_class: p.status_class.clone(),
                                        cols: p.cols.clone(),
                                        age: p.age.clone(),
                                    };
                                    rsx! {
                                        div {
                                            class: "sp-row",
                                            onclick: move |_| {
                                                if let Some(h) = on_open {
                                                    h.call(OpenReq { target: t.clone(), tab: DetailTab::Summary });
                                                }
                                            },
                                            span { class: "sp-dot {p.status_class}" }
                                            span { class: "sp-name", "{p.name}" }
                                            span { class: "sp-meta", "{ready}" }
                                            span { class: "sp-age", "{p.age}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Extract `data` / `binaryData` / `stringData` key→value pairs from a manifest.
fn parse_data_entries(yaml: &str) -> Vec<(String, String)> {
    let Ok(v): Result<serde_yaml::Value, _> = serde_yaml::from_str(yaml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for field in ["data", "stringData", "binaryData"] {
        if let Some(m) = v[field].as_mapping() {
            for (k, val) in m {
                if let (Some(k), Some(val)) = (k.as_str(), val.as_str()) {
                    out.push((k.to_string(), val.to_string()));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Decode a base64 secret value to UTF-8 (lossy); returns the input on failure.
fn decode_b64(s: &str) -> String {
    use base64::Engine;
    match base64::engine::general_purpose::STANDARD.decode(s.trim()) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => s.to_string(),
    }
}

/// Copy text to the system clipboard via the webview.
fn copy_to_clipboard(text: &str) {
    let json = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    dioxus::document::eval(&format!("navigator.clipboard.writeText({json});"));
}

/// A copy-to-clipboard button that flips its icon to a checkmark for 2s on
/// success (animated pop), then reverts. `class` is merged with `copy-btn`.
#[component]
fn CopyButton(
    text: String,
    #[props(default)] class: String,
    #[props(default)] tip: Option<String>,
    #[props(default)] label: Option<String>,
) -> Element {
    let mut copied = use_signal(|| false);
    let cls = format!("{class} copy-btn{}", if copied() { " copied" } else { "" });
    rsx! {
        button {
            class: "{cls}",
            "data-tip": tip,
            onclick: move |_| {
                copy_to_clipboard(&text);
                copied.set(true);
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    copied.set(false);
                });
            },
            span { class: "copy-ico",
                if copied() { {icon("i-check", "1.7")} } else { {icon("i-copy", "1.7")} }
            }
            if let Some(l) = label { "{l}" }
        }
    }
}

/// Data tab for ConfigMaps (plain) and Secrets (masked by default, per-key
/// reveal, base64-decoded on reveal).
#[component]
fn DataTab(
    kind_name: String,
    manifest: Signal<Option<String>>,
    manifest_err: Signal<Option<String>>,
) -> Element {
    let mut revealed = use_signal(BTreeSet::<String>::new);
    let is_secret = kind_name == "Secret";
    let m = manifest();
    let entries = m.as_deref().map(parse_data_entries).unwrap_or_default();

    rsx! {
        div { class: "sum",
            if is_secret {
                div { class: "stale-banner",
                    span { class: "badge-dot", style: "background: var(--status-pending)" }
                    "Values are masked by default. Reveal decodes base64 — handle with care."
                }
            }
            if m.is_none() && manifest_err().is_none() {
                div { class: "detail-loading", "Loading…" }
            } else if let Some(err) = manifest_err() {
                div { class: "detail-empty", "Couldn't load: {err}" }
            } else if entries.is_empty() {
                div { class: "detail-empty", "No data keys." }
            }
            div { class: "sum-group",
                for (k, raw) in entries {
                    {
                        let shown = !is_secret || revealed().contains(&k);
                        let value = if is_secret { decode_b64(&raw) } else { raw.clone() };
                        let kk = k.clone();
                        let copy_val = value.clone();
                        rsx! {
                            div { class: "data-row",
                                div { class: "data-head",
                                    span { class: "data-key", "{k}" }
                                    if is_secret {
                                        button {
                                            class: "data-btn",
                                            onclick: move |_| {
                                                let mut r = revealed.write();
                                                if !r.remove(&kk) { r.insert(kk.clone()); }
                                            },
                                            if shown { {icon("i-eye-off", "1.7")} } else { {icon("i-eye", "1.7")} }
                                        }
                                    }
                                    CopyButton {
                                        text: copy_val,
                                        class: "data-btn data-copy".to_string(),
                                    }
                                }
                                if shown {
                                    pre { class: "data-val", "{value}" }
                                } else {
                                    div { class: "data-val masked", "•••••••••••• · {raw.len()} chars" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn YamlTab(
    manifest: Signal<Option<String>>,
    manifest_err: Signal<Option<String>>,
    kind_id: String,
    namespace: String,
    name: String,
) -> Element {
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut search = use_signal(String::new);
    let mut cur = use_signal(|| 0usize);

    let manifest_val = manifest();
    let dirty = editing() && manifest_val.as_deref() != Some(draft().as_str());

    // The displayed text — the draft while editing, the manifest otherwise. Search
    // and highlighting run against this, so they work in both modes.
    let src = if editing() {
        draft()
    } else {
        manifest_val.clone().unwrap_or_default()
    };
    let q = search().to_lowercase();
    let total_matches = if q.is_empty() {
        0
    } else {
        src.to_lowercase().matches(&q).count()
    };
    let cur_i = if total_matches == 0 { 0 } else { cur().min(total_matches - 1) };

    // Jump to a match by global index, scroll it into view. In edit mode the marks
    // live in the (transparent-text) backdrop, so mirror its scroll to the textarea.
    let go = use_callback(move |n: usize| {
        cur.set(n);
        dioxus::document::eval(&format!(
            "const el=document.getElementById('ym{n}'); if(el){{ el.scrollIntoView({{block:'center'}}); \
             const t=document.querySelector('.yaml-edit'), h=document.querySelector('.yaml-hl'); \
             if(t&&h){{ t.scrollTop=h.scrollTop; t.scrollLeft=h.scrollLeft; }} }}"
        ));
    });

    rsx! {
        div { class: "yaml-wrap",
            if let Some(yaml) = manifest_val.clone() {
                {
                    let yaml_edit = yaml.clone();
                    let src_ro = src.clone();
                    let src_hl = src.clone();
                    rsx! {
                div { class: "yaml-toolbar",
                    SearchBox {
                        value: search(),
                        placeholder: "Search YAML…",
                        on_change: move |v| { search.set(v); cur.set(0); },
                        on_enter: move |_| {
                            if total_matches > 0 {
                                go.call((cur_i + 1) % total_matches);
                            }
                        },
                    }
                    if !q.is_empty() {
                        if total_matches > 0 {
                            span { class: "match-count", "{cur_i + 1}/{total_matches}" }
                            button {
                                class: "icon-btn", "data-tip": "Previous", "aria-label": "Previous match",
                                onclick: move |_| go.call((cur_i + total_matches - 1) % total_matches),
                                span { class: "flip180", {icon("i-chev-down", "2")} }
                            }
                            button {
                                class: "icon-btn", "data-tip": "Next", "aria-label": "Next match",
                                onclick: move |_| go.call((cur_i + 1) % total_matches),
                                {icon("i-chev-down", "2")}
                            }
                        } else {
                            span { class: "match-count empty", "No matches" }
                        }
                    }
                    div { class: "spacer" }
                    if editing() {
                        if dirty {
                            span { class: "dirty",
                                span { class: "d" }
                                "Unsaved changes"
                            }
                        }
                        button { class: "btn", onclick: move |_| editing.set(false), "Cancel" }
                        button {
                            class: "btn btn-primary",
                            onclick: {
                                let (ns, nm) = (namespace.clone(), name.clone());
                                move |_| {
                                    send_cmd(Cmd::Apply { kind_id: kind_id.clone(), namespace: ns.clone(), name: nm.clone(), yaml: draft() });
                                    // Refresh the manifest after applying.
                                    send_cmd(Cmd::FetchManifest { kind_id: kind_id.clone(), namespace: ns.clone(), name: nm.clone() });
                                    editing.set(false);
                                }
                            },
                            {icon("i-check", "2.2")}
                            "Apply"
                        }
                    } else {
                        button {
                            class: "btn",
                            onclick: move |_| { draft.set(yaml_edit.clone()); editing.set(true); },
                            {icon("i-yaml", "1.7")}
                            "Edit"
                        }
                    }
                }
                if editing() {
                    div { class: "yaml-editor",
                        pre { class: "yaml-hl",
                            {
                                let counter = std::cell::Cell::new(0usize);
                                rsx! {
                                    for (i, line) in src_hl.lines().enumerate() {
                                        if i > 0 { "\n" }
                                        for (cls, txt) in highlight_yaml(line) {
                                            if q.is_empty() {
                                                span { class: "{cls}", "{txt}" }
                                            } else {
                                                span { class: "{cls}",
                                                    for (is_m, seg) in highlight_match(&txt, &q) {
                                                        if is_m {
                                                            {
                                                                let mi = counter.get();
                                                                counter.set(mi + 1);
                                                                let mc = if mi == cur_i { "ymatch cur" } else { "ymatch" };
                                                                rsx! { span { id: "ym{mi}", class: "{mc}", "{seg}" } }
                                                            }
                                                        } else {
                                                            "{seg}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        textarea {
                            class: "yaml-edit",
                            autocomplete: "off",
                            autocapitalize: "off",
                            spellcheck: "false",
                            "autocorrect": "off",
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                            onscroll: move |_| {
                                dioxus::document::eval(
                                    "const t=document.querySelector('.yaml-edit'), h=document.querySelector('.yaml-hl'); \
                                     if(t&&h){ h.scrollTop=t.scrollTop; h.scrollLeft=t.scrollLeft; }",
                                );
                            },
                        }
                    }
                } else {
                    div { class: "code",
                        {
                            let counter = std::cell::Cell::new(0usize);
                            rsx! {
                                for (i, line) in src_ro.lines().enumerate() {
                                    div { class: "ln", key: "{i}",
                                        span { class: "lno", "{i + 1}" }
                                        code {
                                            for (cls, txt) in highlight_yaml(line) {
                                                if q.is_empty() {
                                                    span { class: "{cls}", "{txt}" }
                                                } else {
                                                    span { class: "{cls}",
                                                        for (is_m, seg) in highlight_match(&txt, &q) {
                                                            if is_m {
                                                                {
                                                                    let mi = counter.get();
                                                                    counter.set(mi + 1);
                                                                    let mc = if mi == cur_i { "ymatch cur" } else { "ymatch" };
                                                                    rsx! { span { id: "ym{mi}", class: "{mc}", "{seg}" } }
                                                                }
                                                            } else {
                                                                "{seg}"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                    }
                }
            } else if let Some(err) = manifest_err() {
                div { class: "detail-empty", "Couldn't load manifest: {err}" }
            } else {
                div { class: "detail-loading", "Loading manifest…" }
            }
        }
    }
}

/// Split a message into `(is_match, text)` segments for inline highlighting of
/// a (lowercased) query. Operates on byte offsets — fine for ASCII log output;
/// falls back gracefully if a unicode slice isn't on a char boundary.
fn highlight_match(msg: &str, q: &str) -> Vec<(bool, String)> {
    if q.is_empty() {
        return vec![(false, msg.to_string())];
    }
    let hay = msg.to_lowercase();
    let qlen = q.len();
    let mut out: Vec<(bool, String)> = Vec::new();
    let (mut last, mut from) = (0usize, 0usize);
    while let Some(rel) = hay[from..].find(q) {
        let m = from + rel;
        if m > last {
            if let Some(s) = msg.get(last..m) {
                out.push((false, s.to_string()));
            }
        }
        if let Some(s) = msg.get(m..m + qlen) {
            out.push((true, s.to_string()));
        }
        last = m + qlen;
        from = m + qlen;
    }
    if let Some(s) = msg.get(last..) {
        out.push((false, s.to_string()));
    }
    out
}

/// Log-level class for a message line (heuristic).
fn log_level_class(msg: &str) -> &'static str {
    let m = msg.to_lowercase();
    if m.contains("error") || m.contains("fatal") || m.contains("panic") {
        "lm lvl-err"
    } else if m.contains("warn") {
        "lm lvl-warn"
    } else if m.contains("info") {
        "lm lvl-info"
    } else {
        "lm"
    }
}

/// Color for a per-pod log source index, from the cluster-accent palette.
fn log_source_color(idx: u8) -> String {
    format!("var(--cluster-{})", (idx % 8) + 1)
}

/// SGR text style carried while parsing ANSI escape codes in log output.
#[derive(Clone, Default, PartialEq)]
struct AnsiStyle {
    fg: Option<&'static str>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

impl AnsiStyle {
    /// Inline CSS for this run; empty when there's nothing to style.
    fn css(&self) -> String {
        let mut s = String::new();
        if let Some(c) = self.fg {
            s.push_str("color:");
            s.push_str(c);
            s.push(';');
        }
        if self.bold {
            s.push_str("font-weight:600;");
        }
        if self.dim {
            s.push_str("opacity:0.65;");
        }
        if self.italic {
            s.push_str("font-style:italic;");
        }
        if self.underline {
            s.push_str("text-decoration:underline;");
        }
        s
    }
}

/// Map a standard / bright ANSI fg code to a theme color token.
fn ansi_fg(code: u32) -> Option<&'static str> {
    Some(match code {
        30 | 90 => "var(--fg-faint)",       // black / bright black
        31 | 91 => "var(--status-failed)",  // red
        32 | 92 => "var(--status-running)", // green
        33 | 93 => "var(--status-pending)", // yellow
        34 | 94 => "var(--accent-fg)",      // blue
        35 | 95 => "var(--cluster-5)",      // magenta
        36 | 96 => "var(--cluster-1)",      // cyan
        37 | 97 => "var(--fg-strong)",      // white
        _ => return None,
    })
}

/// Apply one SGR parameter list (`ESC[<params>m`) to a style.
fn apply_sgr(style: &mut AnsiStyle, params: &str) {
    let codes: Vec<u32> = params
        .split(';')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();
    if codes.is_empty() {
        *style = AnsiStyle::default(); // bare ESC[m == reset
        return;
    }
    let mut k = 0;
    while k < codes.len() {
        match codes[k] {
            0 => *style = AnsiStyle::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            39 => style.fg = None,
            38 | 48 => {
                // Extended color: skip its args (5;n or 2;r;g;b) — left uncolored.
                match codes.get(k + 1) {
                    Some(5) => k += 2,
                    Some(2) => k += 4,
                    _ => {}
                }
            }
            c @ (30..=37 | 90..=97) => style.fg = ansi_fg(c),
            _ => {}
        }
        k += 1;
    }
}

/// Parse a string with ANSI SGR escape codes into styled `(style, text)` runs.
/// Non-SGR escape sequences (cursor moves, etc.) are dropped.
fn ansi_parse(s: &str) -> Vec<(AnsiStyle, String)> {
    let mut out: Vec<(AnsiStyle, String)> = Vec::new();
    let mut style = AnsiStyle::default();
    let mut buf = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                let mut params = String::new();
                let mut final_byte = None;
                while let Some(&pc) = it.peek() {
                    it.next();
                    if pc.is_ascii_digit() || pc == ';' {
                        params.push(pc);
                    } else {
                        final_byte = Some(pc);
                        break;
                    }
                }
                if final_byte == Some('m') {
                    if !buf.is_empty() {
                        out.push((style.clone(), std::mem::take(&mut buf)));
                    }
                    apply_sgr(&mut style, &params);
                }
                // other CSI finals (cursor moves, etc.) are consumed and dropped
            } else {
                it.next(); // drop the char after a non-CSI escape
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push((style, buf));
    }
    out
}

/// Strip all ANSI escape sequences — used for plain-text matching/filtering.
fn ansi_strip(s: &str) -> String {
    ansi_parse(s).into_iter().map(|(_, t)| t).collect()
}

#[component]
fn LogsTab(
    logs: Signal<Vec<LogEntry>>,
    kind_id: String,
    namespace: String,
    name: String,
    containers: Vec<String>,
) -> Element {
    let mut search = use_signal(String::new);
    let mut exclude = use_signal(String::new);
    let mut follow = use_signal(|| true);
    let mut wrap = use_signal(|| true);
    // None = all containers (default).
    let mut selected = use_signal(|| None::<String>);
    let mut ctr_open = use_signal(|| false);

    let current = selected();
    let current_label = current.clone().unwrap_or_else(|| "All containers".into());

    // Auto-scroll to the bottom on new lines while Follow is on.
    use_effect(move || {
        let _ = logs.read().len(); // subscribe to new lines
        if follow() {
            dioxus::document::eval(
                "requestAnimationFrame(()=>{const e=document.querySelector('.logview'); if(e) e.scrollTop=e.scrollHeight;});",
            );
        }
    });

    let lines = logs();
    let total = lines.len();
    let q = search().to_lowercase();
    let exq = exclude().to_lowercase();
    let shown: Vec<(usize, LogEntry)> = lines
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let l = ansi_strip(&e.line).to_lowercase();
            (q.is_empty() || l.contains(&q)) && (exq.is_empty() || !l.contains(&exq))
        })
        .map(|(i, e)| (i, e.clone()))
        .collect();
    let shown_count = shown.len();
    let logview_class = if wrap() { "logview" } else { "logview nowrap" };

    // Distinct pod sources (for the legend) — only meaningful for merged streams.
    let mut sources: Vec<(u8, String)> = Vec::new();
    for e in lines.iter() {
        if !sources.iter().any(|(_, s)| s == &e.source) {
            sources.push((e.idx, e.source.clone()));
        }
    }
    let multi_source = sources.len() > 1;

    rsx! {
        div { class: "logs-wrap",
            div { class: "logs-toolbar",
                if containers.len() > 1 {
                    div { class: "ns-wrap",
                        button {
                            class: "toggle",
                            onclick: move |_| ctr_open.toggle(),
                            {icon("i-workloads", "1.6")}
                            "{current_label}"
                            {icon("i-chev-down", "2")}
                        }
                        if ctr_open() {
                            div { class: "menu-scrim", onclick: move |_| ctr_open.set(false) }
                            div { class: "menu under",
                                div { class: "menu-head", "Container" }
                                {
                                    let (ns, nm, kid) = (namespace.clone(), name.clone(), kind_id.clone());
                                    rsx! {
                                        div {
                                            class: if current.is_none() { "menu-item hl" } else { "menu-item" },
                                            onclick: move |_| {
                                                selected.set(None);
                                                ctr_open.set(false);
                                                send_cmd(Cmd::StartLogs {
                                                    kind_id: kid.clone(),
                                                    namespace: ns.clone(),
                                                    name: nm.clone(),
                                                    container: None,
                                                });
                                            },
                                            span { "All containers" }
                                            if current.is_none() {
                                                span { class: "check", {icon("i-check", "3")} }
                                            }
                                        }
                                    }
                                }
                                div { class: "menu-sep" }
                                for c in containers.clone() {
                                    {
                                        let is_active = current.as_deref() == Some(c.as_str());
                                        let pick = c.clone();
                                        let ns = namespace.clone();
                                        let nm = name.clone();
                                        let kid = kind_id.clone();
                                        rsx! {
                                            div {
                                                class: if is_active { "menu-item hl" } else { "menu-item" },
                                                onclick: move |_| {
                                                    selected.set(Some(pick.clone()));
                                                    ctr_open.set(false);
                                                    send_cmd(Cmd::StartLogs {
                                                        kind_id: kid.clone(),
                                                        namespace: ns.clone(),
                                                        name: nm.clone(),
                                                        container: Some(pick.clone()),
                                                    });
                                                },
                                                span { "{c}" }
                                                if is_active {
                                                    span { class: "check", {icon("i-check", "3")} }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                SearchBox {
                    value: search(),
                    placeholder: "Search logs…",
                    on_change: move |v| search.set(v),
                }
                SearchBox {
                    value: exclude(),
                    placeholder: "Exclude…",
                    class: "search exclude",
                    icon_id: "i-eye-off",
                    on_change: move |v| exclude.set(v),
                }
                button {
                    class: if follow() { "toggle on" } else { "toggle" },
                    onclick: move |_| follow.toggle(),
                    span { class: "sw" }
                    "Follow"
                }
                button {
                    class: if wrap() { "toggle on" } else { "toggle" },
                    onclick: move |_| wrap.toggle(),
                    {icon("i-wrap", "1.8")}
                    "Wrap"
                }
            }
            if multi_source {
                div { class: "pod-legend",
                    for (idx, src) in sources.iter().cloned() {
                        span { class: "pl",
                            span { class: "b", style: "background: {log_source_color(idx)}" }
                            "{src}"
                        }
                    }
                }
            }
            div { class: "{logview_class}",
                if total == 0 {
                    div { class: "detail-loading", "Waiting for log output…" }
                } else if shown_count == 0 {
                    div { class: "detail-loading", "No lines match “{search}”." }
                }
                for (i, entry) in shown {
                    {
                        let (ts, msg) = split_log(&entry.line);
                        let msg = msg.to_string();
                        // Heuristic level color uses the plain text (no escape codes).
                        let lm_class = log_level_class(&ansi_strip(&msg));
                        let color = log_source_color(entry.idx);
                        rsx! {
                            div { class: "logline", key: "{i}",
                                span { class: "lt", "{ts}" }
                                span { class: "lsrc", style: "background: {color}" }
                                span { class: "{lm_class}",
                                    // Each ANSI run carries its own color/weight; search
                                    // matches are highlighted within the run.
                                    for (st, run) in ansi_parse(&msg) {
                                        span { style: "{st.css()}",
                                            for (hit, seg) in highlight_match(&run, &q) {
                                                if hit {
                                                    span { class: "hl", "{seg}" }
                                                } else {
                                                    "{seg}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "logs-foot",
                if follow() {
                    span { class: "live",
                        span { class: "d" }
                        "Live"
                    }
                } else {
                    span { "Paused" }
                }
                span {
                    if q.is_empty() { "{total} lines" } else { "{shown_count} of {total} lines" }
                }
            }
        }
    }
}

/// Loads xterm.js into the webview, mounts a terminal on `#kterm`, forwards
/// keystrokes via `dioxus.send`, and signals READY once it's open.
const XTERM_SETUP: &str = r#"
(async () => {
  // Wait for the (vendored) libs loaded at startup.
  for (let i = 0; i < 100 && typeof Terminal === 'undefined'; i++) {
    await new Promise(r => setTimeout(r, 50));
  }
  const el = document.getElementById('kterm');
  if (!el) return;
  if (typeof Terminal === 'undefined') { el.textContent = 'terminal failed to load'; return; }
  el.innerHTML = '';
  const term = new Terminal({ fontFamily: 'JetBrains Mono, ui-monospace, monospace', fontSize: 12,
    cursorBlink: true, theme: { background: '#0d1117', foreground: '#e6edf3' } });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(el);
  try { fit.fit(); } catch (e) {}
  window.__kterm = term;
  window.__kfit = fit;
  term.onData(d => dioxus.send(d));
  term.onResize(({ cols, rows }) => dioxus.send('R:' + cols + ':' + rows));
  // Refit on panel resize.
  new ResizeObserver(() => { try { fit.fit(); } catch (e) {} }).observe(el);
  term.focus();
  dioxus.send('__KOMPASS_READY__');
  // Re-emit size after the session is attached so the pty matches.
  setTimeout(() => { try { fit.fit(); } catch (e) {} }, 350);
})();
"#;

#[component]
fn ExecTab(namespace: String, name: String) -> Element {
    use_hook(move || {
        let (ns, nm) = (namespace.clone(), name.clone());
        spawn(async move {
            let mut eval = dioxus::document::eval(XTERM_SETUP);
            while let Ok(d) = eval.recv::<String>().await {
                if d == "__KOMPASS_READY__" {
                    send_cmd(Cmd::StartExec {
                        namespace: ns.clone(),
                        name: nm.clone(),
                        container: None,
                    });
                } else if let Some(dims) = d.strip_prefix("R:") {
                    if let Some((c, r)) = dims.split_once(':') {
                        if let (Ok(cols), Ok(rows)) = (c.parse::<u16>(), r.parse::<u16>()) {
                            send_cmd(Cmd::ExecResize { cols, rows });
                        }
                    }
                } else {
                    send_cmd(Cmd::ExecInput(d));
                }
            }
        });
    });
    use_drop(|| {
        send_cmd(Cmd::StopExec);
        dioxus::document::eval(
            "if(window.__kterm){try{window.__kterm.dispose();}catch(e){} window.__kterm=null;}",
        );
    });
    rsx! {
        div { class: "exec-wrap",
            div { id: "kterm", class: "exec-term" }
        }
    }
}

/// Merged logs across a bulk-selected set of pods (reuses the LogsTab view).
#[component]
fn MultiLogPanel(logs: Signal<Vec<LogEntry>>, label: String, on_close: EventHandler<()>) -> Element {
    rsx! {
        div { class: "detail open enter",
            div { class: "detail-head",
                div { class: "detail-toprow",
                    div { class: "detail-kind", "LOGS" }
                    div { class: "detail-headctrls",
                        button {
                            class: "icon-btn tip",
                            "data-tip": "Close",
                            onclick: move |_| on_close.call(()),
                            {icon("i-x", "1.8")}
                        }
                    }
                }
                div { class: "detail-title", "{label}" }
            }
            div { class: "detail-tabs",
                button { class: "detail-tab active", {icon("i-logs", "1.7")} "Logs" }
            }
            div { class: "detail-body",
                LogsTab {
                    key: "multi",
                    logs,
                    kind_id: String::new(),
                    namespace: String::new(),
                    name: String::new(),
                    containers: Vec::new(),
                }
            }
        }
    }
}

#[component]
fn OverviewScreen(data: Option<OverviewData>, ctx: String, on_refresh: EventHandler<()>) -> Element {
    let Some(d) = data else {
        return rsx! {
            div { class: "list-head", div { class: "list-titlerow", span { class: "list-title", "Overview" } } }
            div { class: "kload",
                div { class: "kspin" }
                span { "Gathering cluster snapshot…" }
            }
        };
    };

    let pct = |n: usize, total: usize| if total == 0 { 0.0 } else { n as f64 / total as f64 * 100.0 };
    let wl_total = (d.pods_running + d.pods_pending + d.pods_failed + d.pods_succeeded).max(1);
    let (run_w, pend_w, fail_w, done_w) = (
        pct(d.pods_running, wl_total),
        pct(d.pods_pending, wl_total),
        pct(d.pods_failed, wl_total),
        pct(d.pods_succeeded, wl_total),
    );
    let cpu_used = d.cpu_used_milli as f64 / 1000.0;
    let cpu_cap = d.cpu_cap_milli as f64 / 1000.0;
    let cpu_pct = if d.cpu_cap_milli > 0 { d.cpu_used_milli as f64 / d.cpu_cap_milli as f64 * 100.0 } else { 0.0 };
    let gib = |b: i64| b as f64 / (1024.0 * 1024.0 * 1024.0);
    let mem_pct = if d.mem_cap_bytes > 0 { d.mem_used_bytes as f64 / d.mem_cap_bytes as f64 * 100.0 } else { 0.0 };

    rsx! {
        div { class: "list-head", style: "padding-bottom:0",
            div { class: "list-titlerow",
                span { class: "list-title", "Overview" }
                div { class: "list-sub",
                    button { class: "btn", onclick: move |_| on_refresh.call(()),
                        {icon("i-refresh", "1.8")} "Refresh"
                    }
                }
            }
        }
        div { class: "ov",
            div { class: "ov-grid",
                div { class: "ov-hero",
                    span { class: "glyph",
                        {kompass_mark("")}
                    }
                    div { class: "hid",
                        span { class: "hname", "{ctx}" }
                        if d.ver_loaded {
                            span { class: "hctx", "Kubernetes {d.version}" }
                        } else {
                            span { class: "skel", style: "height:12px;width:180px;margin-top:6px" }
                        }
                    }
                    div { class: "hmeta",
                        div { class: "hstat", span { class: "l", "Namespaces" }
                            if d.ns_loaded { span { class: "v tnum", "{d.namespaces}" } } else { span { class: "skel", style: "height:18px;width:32px" } }
                        }
                        div { class: "hstat", span { class: "l", "Nodes" }
                            if d.nodes_loaded { span { class: "v tnum", "{d.nodes_ready}/{d.nodes_total}" } } else { span { class: "skel", style: "height:18px;width:40px" } }
                        }
                    }
                }

                div { class: "ov-card c3",
                    div { class: "ctitle", "Pods" }
                    if d.pods_loaded {
                        div { class: "ov-bignum", "{d.pods_running}" span { class: "unit", "running" } }
                        div { class: "ov-sub", "{d.pods_pending} pending · {d.pods_failed} failed" }
                    } else {
                        span { class: "skel", style: "height:30px;width:72px;display:block;margin:4px 0" }
                        span { class: "skel", style: "height:11px;width:120px;display:block" }
                    }
                }
                div { class: "ov-card c3",
                    div { class: "ctitle", "Deployments" }
                    if d.deps_loaded {
                        div { class: "ov-bignum", "{d.deployments_total}" }
                        div { class: "ov-sub", "{d.deployments_available} available" }
                    } else {
                        span { class: "skel", style: "height:30px;width:56px;display:block;margin:4px 0" }
                        span { class: "skel", style: "height:11px;width:90px;display:block" }
                    }
                }
                div { class: "ov-card c3",
                    div { class: "ctitle", "Nodes ready" }
                    if d.nodes_loaded {
                        div { class: "ov-bignum", "{d.nodes_ready}" span { class: "unit", "/ {d.nodes_total}" } }
                        div { class: "ov-sub",
                            if d.nodes_ready == d.nodes_total { span { class: "up", "All healthy" } } else { "{d.nodes_total - d.nodes_ready} not ready" }
                        }
                    } else {
                        span { class: "skel", style: "height:30px;width:64px;display:block;margin:4px 0" }
                        span { class: "skel", style: "height:11px;width:80px;display:block" }
                    }
                }
                div { class: "ov-card c3",
                    div { class: "ctitle", "Services" }
                    if d.svcs_loaded {
                        div { class: "ov-bignum", "{d.services_total}" }
                        div { class: "ov-sub", "{d.services_lb} LoadBalancer" }
                    } else {
                        span { class: "skel", style: "height:30px;width:56px;display:block;margin:4px 0" }
                        span { class: "skel", style: "height:11px;width:100px;display:block" }
                    }
                }

                div { class: "ov-card c8",
                    div { class: "ctitle", "Workload health" }
                    if d.pods_loaded {
                        div { class: "health-bar",
                            span { style: "width:{run_w}%;background:var(--status-running)" }
                            span { style: "width:{pend_w}%;background:var(--status-pending)" }
                            span { style: "width:{fail_w}%;background:var(--status-failed)" }
                            span { style: "width:{done_w}%;background:var(--status-neutral)" }
                        }
                        div { class: "health-legend",
                            div { class: "li", span { class: "d", style: "background:var(--status-running)" } span { class: "ln", "Running" } span { class: "lv", "{d.pods_running}" } }
                            div { class: "li", span { class: "d", style: "background:var(--status-pending)" } span { class: "ln", "Pending" } span { class: "lv", "{d.pods_pending}" } }
                            div { class: "li", span { class: "d", style: "background:var(--status-failed)" } span { class: "ln", "Failed" } span { class: "lv", "{d.pods_failed}" } }
                            div { class: "li", span { class: "d", style: "background:var(--status-neutral)" } span { class: "ln", "Completed" } span { class: "lv", "{d.pods_succeeded}" } }
                        }
                    } else {
                        span { class: "skel", style: "height:8px;width:100%;display:block;margin:8px 0" }
                        span { class: "skel", style: "height:11px;width:60%;display:block" }
                    }
                }
                div { class: "ov-card c4",
                    div { class: "ctitle", "Cluster usage" }
                    if d.nodes_loaded {
                        div { class: "usage-row",
                            div { class: "usage-top", span { class: "un", "CPU" } span { class: "uv", b { "{cpu_used:.1}" } " / {cpu_cap:.0} cores · {cpu_pct:.0}%" } }
                            div { class: "meter", span { style: "width:{cpu_pct}%" } }
                        }
                        div { class: "usage-row",
                            div { class: "usage-top", span { class: "un", "Memory" } span { class: "uv", b { "{gib(d.mem_used_bytes):.0}" } " / {gib(d.mem_cap_bytes):.0} GiB · {mem_pct:.0}%" } }
                            div { class: "meter", span { class: if mem_pct > 80.0 { "warn" } else { "" }, style: "width:{mem_pct}%" } }
                        }
                    } else {
                        span { class: "skel", style: "height:8px;width:100%;display:block;margin:8px 0" }
                        span { class: "skel", style: "height:8px;width:100%;display:block;margin:8px 0" }
                    }
                }

                div { class: "ov-card c8",
                    div { class: "ctitle", "Recent warning events" }
                    if !d.events_loaded {
                        for _ in 0..3 {
                            div { class: "ev-row",
                                span { class: "skel", style: "height:11px;width:60%;display:block" }
                            }
                        }
                    } else if d.events.is_empty() {
                        div { class: "ov-sub", style: "padding: var(--sp-5) 0", "No recent warnings." }
                    } else {
                        div { class: "ev-list",
                            for e in d.events.iter() {
                                div { class: "ev-row",
                                    span { class: "edot", style: "background:var(--status-pending)" }
                                    div { class: "emain",
                                        div { class: "ereason", "{e.reason}" }
                                        div { class: "emsg", "{e.message} · " span { class: "obj", "{e.object}" } }
                                    }
                                    span { class: "eage", "{e.age} ago" }
                                }
                            }
                        }
                    }
                }
                div { class: "ov-card c4",
                    div { class: "ctitle", "Nodes" }
                    if !d.nodes_loaded {
                        for _ in 0..3 {
                            div { class: "node-row",
                                span { class: "skel", style: "height:11px;width:100%;display:block" }
                            }
                        }
                    } else {
                        for n in d.nodes.iter() {
                            div { class: "node-row",
                                span { class: "nn", span { class: "d", style: if n.ready { "background:var(--status-running)" } else { "background:var(--status-failed)" } } "{n.name}" }
                                div { class: "nmeter", span { class: "nl", "cpu {n.cpu_pct}%" } div { class: "meter", style: "height:5px", span { style: "width:{n.cpu_pct}%" } } }
                                div { class: "nmeter", span { class: "nl", "mem {n.mem_pct}%" } div { class: "meter", style: "height:5px", span { class: if n.mem_pct > 80 { "warn" } else { "" }, style: "width:{n.mem_pct}%" } } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Calm shimmer skeleton for the cold-load state (no cache yet).
#[component]
fn SkeletonTable() -> Element {
    rsx! {
        div { class: "table-wrap",
            table { class: "table",
                tbody {
                    for i in 0..9 {
                        tr { key: "{i}",
                            td { class: "col-check", span { class: "skel", style: "width:14px" } }
                            td { span { class: "skel", style: "width:{180 - (i % 4) * 24}px" } }
                            td { span { class: "skel", style: "width:90px" } }
                            td { span { class: "skel", style: "width:70px" } }
                            td { span { class: "skel", style: "width:34px" } }
                            td { span { class: "skel", style: "width:22px" } }
                            td { span { class: "skel", style: "width:30px" } }
                            td {}
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_strip_removes_codes() {
        assert_eq!(ansi_strip("\x1b[32mGET\x1b[0m /status"), "GET /status");
        assert_eq!(ansi_strip("plain"), "plain");
        // non-SGR CSI (cursor move) is dropped too
        assert_eq!(ansi_strip("a\x1b[2Kb"), "ab");
    }

    #[test]
    fn ansi_parse_colors_runs() {
        let runs = ansi_parse("\x1b[31mERR\x1b[0m ok");
        assert_eq!(runs[0].1, "ERR");
        assert_eq!(runs[0].0.fg, Some("var(--status-failed)"));
        // after reset, the trailing run has no color
        assert_eq!(runs.last().unwrap().1, " ok");
        assert_eq!(runs.last().unwrap().0.fg, None);
    }

    #[test]
    fn ansi_parse_bold_and_extended_skip() {
        let runs = ansi_parse("\x1b[1;32mX\x1b[0m");
        assert!(runs[0].0.bold);
        assert_eq!(runs[0].0.fg, Some("var(--status-running)"));
        // 256-color and truecolor args are consumed without coloring or panic
        let runs = ansi_parse("\x1b[38;5;200mA\x1b[38;2;1;2;3mB\x1b[0m");
        assert_eq!(ansi_strip("\x1b[38;5;200mA\x1b[38;2;1;2;3mB\x1b[0m"), "AB");
        assert!(runs.iter().all(|(s, _)| s.fg.is_none()));
    }

    #[test]
    fn highlight_match_splits() {
        assert_eq!(
            highlight_match("Hello", "ell"),
            vec![(false, "H".into()), (true, "ell".into()), (false, "o".into())]
        );
        // case-insensitive, original casing preserved
        assert_eq!(highlight_match("ERROR", "err"), vec![(true, "ERR".into()), (false, "OR".into())]);
        // empty query → single non-match segment
        assert_eq!(highlight_match("abc", ""), vec![(false, "abc".into())]);
    }

    #[test]
    fn log_level_heuristic() {
        assert_eq!(log_level_class("panic: boom"), "lm lvl-err");
        assert_eq!(log_level_class("WARN retrying"), "lm lvl-warn");
        assert_eq!(log_level_class("info started"), "lm lvl-info");
        assert_eq!(log_level_class("just a line"), "lm");
    }

    #[test]
    fn sort_key_id_roundtrips() {
        // includes the new CPU/Mem keys
        for k in [
            SortKey::Name,
            SortKey::Namespace,
            SortKey::Status,
            SortKey::Age,
            SortKey::Cpu,
            SortKey::Mem,
            SortKey::Col(3),
        ] {
            assert!(SortKey::from_id(&k.id()) == k, "roundtrip failed for {:?}", k.id());
        }
        assert_eq!(SortKey::Cpu.id(), "cpu");
        assert_eq!(SortKey::Mem.id(), "mem");
        assert!(SortKey::from_id("cpu") == SortKey::Cpu);
        assert!(SortKey::from_id("mem") == SortKey::Mem);
        // unknown id falls back to Name
        assert!(SortKey::from_id("bogus") == SortKey::Name);
    }

    #[test]
    fn parse_age_secs_units() {
        assert_eq!(parse_age_secs("45s"), 45);
        assert_eq!(parse_age_secs("5m"), 300);
        assert_eq!(parse_age_secs("2h"), 7200);
        assert_eq!(parse_age_secs("3d"), 259_200);
        assert_eq!(parse_age_secs("-"), 0);
        assert_eq!(parse_age_secs(""), 0);
    }

    #[test]
    fn col_cmp_numeric_vs_string() {
        use std::cmp::Ordering;
        // numeric lead → compared as numbers, not lexically ("2" < "10")
        assert_eq!(col_cmp("2/3", "10/10"), Ordering::Less);
        assert_eq!(col_cmp("5", "5"), Ordering::Equal);
        // non-numeric → string compare
        assert_eq!(col_cmp("ClusterIP", "LoadBalancer"), Ordering::Less);
    }

    #[test]
    fn label_from_id_titlecases_plural() {
        assert_eq!(label_from_id("deployments.apps"), "Deployments");
        assert_eq!(label_from_id("pods"), "Pods");
        assert_eq!(label_from_id("certificates.cert-manager.io"), "Certificates");
    }

    #[test]
    fn is_newer_compares_versions() {
        assert!(is_newer("v1.0.4", "1.0.3"));
        assert!(is_newer("1.1.0", "1.0.9"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.0.3", "1.0.3"));
        assert!(!is_newer("v1.0.3", "1.0.3")); // leading v tolerated
        assert!(!is_newer("0.9.0", "1.0.0"));
        // numeric, not lexical: 1.2 < 1.10
        assert!(is_newer("1.10.0", "1.2.0"));
        assert!(!is_newer("1.2.0", "1.10.0"));
        // pre-release suffix ignored
        assert!(!is_newer("1.0.3-rc1", "1.0.3"));
    }

    #[test]
    fn conn_error_tip_detects_auth_exec() {
        let t = conn_error_tip("auth error: unable to run auth exec: No such file or directory (os error 2)");
        assert!(t.contains("auth plugin"));
        assert!(t.contains("To fix"));
        assert!(t.contains("kubelogin"));
        // raw error preserved
        assert!(t.contains("os error 2"));
        // unrelated errors fall through unchanged
        let other = conn_error_tip("connection refused");
        assert_eq!(other, "Connection error: connection refused");
    }

    #[test]
    fn conn_error_tip_detects_expired_creds() {
        let e = "auth error: auth exec command 'AWS_PROFILE=\"dev\" aws --region us-east-1 eks get-token --cluster-name eks-green-dev' failed with status exit status: 255";
        let t = conn_error_tip(e);
        assert!(t.contains("expired or"));
        // extracts the AWS profile into a tailored refresh command
        assert!(t.contains("aws sso login --profile dev"));
    }

    #[test]
    fn split_log_extracts_time() {
        let (ts, msg) = split_log("2026-06-01T12:23:01Z hello world");
        assert_eq!(ts, "12:23:01");
        assert_eq!(msg, "hello world");
        // no space → no timestamp, whole line is the message
        let (ts, msg) = split_log("nospace");
        assert_eq!(ts, "");
        assert_eq!(msg, "nospace");
    }
}
