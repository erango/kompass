# Kompass — Architecture Spec

> Technical design for building Kompass: a beautiful, performant Kubernetes
> desktop app in **Rust + Dioxus**. Pairs with `DESIGN_SPEC.md`.
> Audience: engineers (and Claude Code) implementing v1.

---

## 1. Goals → architectural constraints

The two north stars from the design spec become hard engineering rules:

| North star | Architectural rule |
|---|---|
| Never block on cluster | **All k8s I/O on background tokio tasks.** UI thread only touches in-memory state. Zero `.await` on the render path. |
| Live at scale | **Watch + reflector cache** per resource kind. Push deltas to UI via channels; UI diffs and renders only changes. |
| Instant clicks | **Stale-while-revalidate.** Reads served from in-memory store synchronously; refresh in background. |
| Tiny footprint | One shared tokio runtime; bounded channels; capped log/event buffers; virtualized lists; lazy watches (only watch what's on screen / requested). |

---

## 2. High-level shape

```
┌─────────────────────────────────────────────────────────────┐
│  Dioxus UI (webview, main thread)                            │
│  - components, signals, virtualized lists                    │
│  - reads StoreSnapshot synchronously, renders                │
└───────────▲───────────────────────────────┬─────────────────┘
            │ deltas (channel)               │ commands (channel)
            │                                 ▼
┌───────────┴───────────────────────────────────────────────┐
│  Core engine (tokio runtime, background)                    │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │ ClusterMgr   │  │ Store (cache)│  │ Command executor │   │
│  │ (per-cluster │→ │ reflectors / │  │ apply/patch/del/ │   │
│  │  clients)    │  │ watchers     │  │ scale/logs/exec  │   │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘   │
│         └─────────────────┴───────────────────┘             │
│                      kube-rs (kube, k8s-openapi)            │
└─────────────────────────────────────────────────────────────┘
            │ HTTPS / WebSocket
            ▼
     Kubernetes API server(s)
```

UI and engine communicate **only via message passing** — never shared locks on
the render path. Engine owns all state; UI holds a cheap, cloneable snapshot.

---

## 3. Crate / workspace layout

Cargo workspace, multiple crates for clean boundaries + fast incremental builds:

```
kompass/
├─ Cargo.toml                  # workspace
├─ crates/
│  ├─ kompass-core/            # k8s engine: clients, store, watch, commands
│  │   ├─ cluster.rs           # ClusterManager, per-cluster lifecycle
│  │   ├─ store.rs             # in-memory reflector cache + snapshots
│  │   ├─ watch.rs             # watcher/reflector wiring, delta stream
│  │   ├─ commands.rs          # apply/patch/delete/scale/restart
│  │   ├─ logs.rs              # log streaming
│  │   ├─ exec.rs              # container exec (ws)
│  │   ├─ kinds.rs             # resource kind registry + column defs
│  │   ├─ model.rs             # normalized resource model + status mapping
│  │   └─ error.rs
│  ├─ kompass-ui/              # Dioxus app: components, screens, theme
│  │   ├─ app.rs
│  │   ├─ theme/               # token system (color/type/space/motion)
│  │   ├─ components/          # table, palette, panel, log view, terminal…
│  │   ├─ screens/             # overview, list, detail, secrets…
│  │   └─ state.rs             # UI signals, bridge to core
│  ├─ kompass-config/          # settings, favorites, recents, kubeconfig load
│  └─ kompass-bin/             # desktop entrypoint, wires core+ui, tray
└─ DESIGN_SPEC.md / ARCHITECTURE.md
```

Rationale: `kompass-core` is UI-agnostic and independently testable (mock API
server / recorded fixtures). UI depends on core, never the reverse.

---

## 4. Key dependencies

| Concern | Crate | Notes |
|---|---|---|
| UI | `dioxus` (desktop) | webview render, RSX, signals |
| Async runtime | `tokio` (multi-thread) | one shared runtime for engine |
| k8s client | `kube` (+ `kube-runtime`) | clients, watcher, reflector, Api |
| k8s types | `k8s-openapi` | typed core resources |
| Dynamic/CRD | `kube::api::DynamicObject` + `kube::discovery` | **primary path** — generic-first engine, see §18 |
| Metrics | `metrics.k8s.io` via dynamic API | inline sparklines, degrade if absent |
| Serialization | `serde`, `serde_yaml`, `serde_json` | YAML edit/apply |
| Channels | `tokio::sync` (mpsc/watch/broadcast) | engine↔UI, deltas |
| Errors | `thiserror` (lib), `anyhow` (bin) | |
| Logging | `tracing` + `tracing-subscriber` | structured diagnostics |
| Terminal render | `vt100`/`ansi-to-html` or xterm-in-webview | exec output |

> `kube` auth reads standard kubeconfig (exec plugins, OIDC, client certs) for
> free — critical for real-world multi-cluster.

---

## 5. State & data flow

### 5.1 The Store (per cluster)
- For each **active** resource kind, a `kube-runtime` **reflector** maintains an
  in-memory `Store` of objects, fed by a `watcher` stream.
- On each delta (Applied/Deleted/Restarted), engine updates its authoritative
  state and emits a **normalized delta** to the UI over a channel.
- Snapshots are cheap to clone (`Arc`-backed) so UI reads are O(1) and lock-free.

### 5.2 Stale-while-revalidate
- UI navigates to a kind → engine returns current snapshot **immediately**
  (possibly empty/stale) + marks state `revalidating` and (re)starts the watcher.
- Freshness state per kind/cluster: `Cold | Loading | Live | Revalidating |
  Error(stale)` → maps directly to the four data states in the design spec.

### 5.3 Lazy watches
- Don't watch everything on connect. Watch a kind when the user views it (or it's
  a favorite/overview-critical). Idle watches GC'd after a TTL to bound memory.
- Overview uses lightweight list+counts, upgrades to watch when opened.

### 5.4 Event coalescing
- High-volume watch streams are **debounced/batched** (e.g. flush every N ms) so
  the UI re-renders at a steady cadence, not per-event. Protects 60fps under load.

---

## 6. Multi-cluster

- `ClusterManager` holds a map of `clusterId → ClusterHandle`.
- Each `ClusterHandle`: its own `kube::Client`, its own set of watchers/stores,
  its own task scope, and a **connection state**
  (`Connecting | Live | Unreachable | Unauthorized`).
- Switching cluster = swap active context in UI; background clusters keep their
  favorite/critical watches warm (bounded), others suspend.
- Auto-accent: `hash(clusterName) % palette.len()` → stable accent (matches
  design spec). Computed in core, surfaced in snapshot.

---

## 7. Commands (writes)

All mutations go through `commands.rs`, returning typed results + emitting toasts:

- **Apply / edit**: server-side apply (`PATCH` apply) from edited YAML; validate
  via dry-run (`?dryRun=All`) before real apply when possible.
- **Patch**: strategic/merge patch for scale, labels, annotations.
- **Scale**: scale subresource.
- **Restart**: rollout restart (patch template annotation).
- **Delete**: with propagation policy; **guarded** by confirm dialog (blast-radius
  aware — design spec §10.5).
- Optimistic UI where safe; reconcile against the next watch delta.

---

## 8. Logs

- `kube` `Api::log_stream` with follow; one task per (pod, container).
- Multi-pod tail = merge multiple streams, tag by pod/container (color in UI).
- **Bounded ring buffer** per stream (cap lines + bytes) to protect memory.
- Backpressure: drop-oldest on overflow; UI shows "buffer trimmed" marker.
- UI log view is virtualized; follow/wrap/search/highlight/download client-side.

---

## 9. Exec / terminal

- `kube` `Api::exec` → WebSocket; bidirectional stdin/stdout/stderr.
- Render in a terminal surface in the webview (xterm.js bridged, or a Rust
  vt100 parser → styled spans). Decide during spike; xterm.js is fastest path
  to a real terminal feel.
- Always display **cluster / namespace / pod / container** context (safety).

---

## 10. Secrets (security-sensitive)

- Values **masked by default**; reveal is per-field with friction (design §7.5).
- base64 decode/encode in core; never log secret values (tracing filters).
- Hold decoded secret bytes only transiently; zeroize buffers after use
  (`zeroize` crate). No secret values written to disk/cache.
- Edits use the same guarded apply path with strong confirmation.

---

## 11. Config & persistence (`kompass-config`)

- Loads kubeconfig(s) via `kube`'s standard resolution (+ `$KUBECONFIG`).
- Persists app settings: theme, favorites, recent clusters/namespaces, window
  state, density. Stored in platform config dir (`directories` crate), JSON/TOML.
- **Never** persists secret values or tokens — relies on kubeconfig/exec plugins.

---

## 12. UI ↔ Core bridge

- Core exposes a `CoreHandle`: `command_tx` (UI→core commands) + subscription to
  a `delta_rx` / `watch::Receiver<Snapshot>` (core→UI).
- In Dioxus: a top-level coroutine owns the receivers, pushes into **signals**;
  components subscribe to signals. No blocking calls in components.
- Commands are fire-and-forget with result events surfaced as toasts.
- The tokio runtime runs on a dedicated thread; Dioxus desktop event loop owns
  the main thread. Bridge via channels only.

---

## 13. Performance tactics (checklist)

- Virtualize every large list (resource tables, logs).
- Coalesce watch events; cap re-render frequency.
- Clone-cheap snapshots (`Arc`), structural sharing; diff in UI.
- Lazy + GC'd watches; bounded buffers everywhere.
- Animate only `transform`/`opacity`.
- Cold-start budget: measure; lazy-init non-critical subsystems.
- Memory budget per cluster; suspend background clusters.

---

## 14. Error handling & resilience

- Watcher auto-reconnect with backoff (kube-runtime handles much of this);
  surface `Error(stale)` state, keep showing last-good data.
- Per-cluster auth/connectivity errors isolated — one bad cluster never blocks
  others or the UI.
- All errors `tracing`-logged; user-facing errors are calm, actionable toasts.

---

## 15. Testing

- `kompass-core`: unit + integration against recorded API fixtures / a mock
  server; deterministic delta-stream tests; status-mapping table tests.
- Snapshot tests for normalized models and kind/column registry.
- UI: component-level logic tests; manual + screenshot review for visual states.
- Optional e2e against `kind`/`k3d` in CI for smoke (browse/edit/logs).

---

## 16. v1 build phases

1. **Spike**: Dioxus desktop window + connect one cluster (kube) + list pods
   from a live watch into a plain table. Prove the channel bridge + no-block.
2. **Generic store + discovery**: API discovery → runtime kind registry,
   `DynamicObject` reflector cache keyed by `(gvk, ns, name)`, generic
   normalized model + pluggable status mappers (§18), four freshness states,
   virtualized table.
3. **Detail panel**: summary + YAML view/edit + guarded apply.
4. **Logs + exec**.
5. **Secrets** (mask/reveal/decode, guarded edit).
6. **Multi-cluster**: ClusterManager, switcher, per-cluster accent + states.
7. **Polish**: theme tokens, command palette, motion, footprint pass.

---

## 17. Resolved technical decisions

- **Terminal: xterm.js in webview.** Bridge xterm.js into the Dioxus webview for
  the exec terminal — real terminal fidelity (colors, resize, control sequences)
  with least effort. Accept the one JS dependency for this surface.
- **CRDs: full DynamicObject.** The engine is **generic-first** — built on
  `kube::api::DynamicObject` + API discovery, so any kind (core or CRD) flows
  through one path. Typed `k8s-openapi` structs are used only where they add
  value (status mapping for well-known kinds). See §18 for the ripple.
- **Metrics: inline sparklines if available.** Read `metrics.k8s.io` when
  metrics-server is present; show inline CPU/mem on pods/nodes. **Degrade
  gracefully** (hide, no error) when absent.
- **Packaging/signing: defer to pre-release.** Dev builds run unsigned during
  v1. Stand up macOS notarization + Windows signing when nearing distribution;
  don't slow the build phase.

---

## 18. Generic-first engine (consequence of full DynamicObject)

Choosing full DynamicObject reshapes the core:

- **API discovery** at connect: enumerate all `GroupVersionKind`s the cluster
  serves (`kube::discovery`), build a runtime **kind registry** — no hard-coded
  kind list. CRDs appear automatically.
- **`model.rs` is generic**: normalize from `DynamicObject` (metadata + raw
  `serde_json::Value` spec/status), not from typed structs. A normalized
  `Resource { gvk, namespace, name, age, labels, status, raw }` is the unit the
  store and UI speak.
- **Status mapping is pluggable**: a per-kind mapper turns raw status → semantic
  state + key columns (ready 3/3, restarts, phase). Well-known kinds (Pod,
  Deployment, …) get hand-written mappers; unknown/CRD kinds fall back to a
  generic mapper (conditions / `status.phase` heuristics) and a default column
  set. Mappers are data, easy to extend.
- **Watches are generic**: `watcher` over `Api<DynamicObject>` for any GVK; the
  reflector store is keyed by `(gvk, namespace, name)`.
- **Kind registry drives nav**: the left-nav categories (§6 design) are curated
  groupings over discovered kinds; everything else is reachable via ⌘K / an
  "All kinds" browser.
- Cost: more upfront generic plumbing in phase 2; pays off — logs/exec/edit/apply
  and CRDs all share one code path. Adjust §16 phase 2 to build the discovery +
  generic store + mapper layer first.
