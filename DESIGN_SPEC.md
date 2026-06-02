# Kompass — Design Spec

> A beautiful, fast Kubernetes management & visibility desktop app.
> This document is the design brief for **Claude Design**. It defines product
> intent, design principles, information architecture, screen-by-screen layout,
> component inventory, and the visual system to produce mockups against.

---

## 1. Product in one line

Kompass is a native desktop app that lets engineers **browse, edit, observe, and
operate** Kubernetes clusters — across many clusters at once — with the calm,
premium feel of Linear/Vercel and the responsiveness of a native tool.

Lineage: Lens, Aptakube, Radar (Skyhook), k9s. Kompass aims to be the most
**beautiful and performant** of the family.

---

## 2. The two north stars

Every design decision is judged against these. When they conflict, say so.

### Beautiful
- **Typographic hierarchy first.** The interface is built from type, space, and
  restraint — not borders and boxes. Refined type scale, generous and consistent
  spacing rhythm, a disciplined color system. Premium, calm, confident.
  Reference feel: **Linear, Vercel dashboard, Raycast.**
- **Meticulous dual theme.** Dark and light are *both* first-class, both tuned
  by hand. Theme is part of the brand, not an afterthought toggle. Every color
  is a semantic token; no raw hex in components.

### Performant (and *felt* as performant)
- **Instant interactions.** Sub-100ms on every click and navigation. No spinners
  on data already cached. Optimistic UI where safe.
- **Live at scale.** Real-time watch on resources, log tail, and metrics with no
  jank under high event volume.
- **Never block on the cluster.** The UI must *never* freeze while fetching from
  a cluster. Network is always async/background. Stale-while-revalidate: show
  cached data instantly, refresh in the background, indicate freshness subtly.
- **Tiny footprint.** Low idle memory/CPU, fast cold start. The anti-Electron.

> Design implication: every data surface needs **four visual states** — cached
> (fresh), cached (revalidating), loading-cold (no cache), and error/stale —
> and the transitions between them must be calm, not flashing.

---

## 3. Tech stack (context for the designer)

- **Dioxus** (Rust, RSX, web-like styling) renders to a native desktop webview.
- This means: **web-like styling is available** — flexbox/grid layouts, CSS-style
  tokens, transitions, transforms. Design as you would for a high-end web app.
- Constraint: keep animation GPU-cheap (transform/opacity), avoid heavy DOM.
  Favor virtualized lists for large resource sets.
- Output we want from Claude Design: high-fidelity mockups + a token system
  (color, type, spacing, radius, shadow, motion) we can translate to Dioxus.

---

## 4. Target users & jobs

Primary: **DevOps / SRE / platform** engineers + **app developers** debugging
their own services. Progressive disclosure: simple and calm by default, depth
on demand.

Top jobs (v1):
1. Find a resource fast across namespaces/clusters and read its real state.
2. Edit a resource (YAML or form) and apply safely.
3. View and edit **secrets and resources** with appropriate guardrails.
4. Stream logs (multi-pod tail) and exec into a container.
5. Switch and compare across **multiple clusters / contexts**.

---

## 5. v1 scope

In:
- **Browse & edit** core resources: namespaces, workloads (pods, deployments,
  statefulsets, daemonsets, jobs, cronjobs), services, ingress, configmaps,
  **secrets**, PVC/PV, nodes, events.
- **YAML + form editing**, scale, restart, delete, with confirmation guardrails.
- **Logs & exec**: live stream, multi-pod tail, in-app terminal.
- **Multi-cluster**: connect via kubeconfig, switch contexts, run several at once.

Out (later): full Prometheus dashboards, Helm/release management, RBAC editor,
CRD-rich custom views, plugins.

> Note: metrics were *not* selected for v1. Show lightweight inline health
> (pod ready counts, restart counts, CPU/mem sparklines if metrics-server is
> present) but do **not** design full observability dashboards yet.

---

## 6. Information architecture

```
App
├─ Cluster switcher (global, always reachable)         ← multi-cluster core
├─ Command palette (⌘K) — navigate / act anywhere       ← keyboard-first
├─ Primary nav (left): resource categories
│   ├─ Overview (cluster health snapshot)
│   ├─ Workloads (pods, deployments, …)
│   ├─ Network (services, ingress, endpoints)
│   ├─ Config (configmaps, secrets)
│   ├─ Storage (PVC, PV, storage classes)
│   ├─ Nodes
│   └─ Events
├─ Resource list (center): virtualized table, filter/search, status
└─ Resource detail (right / full): summary, YAML, logs, exec, events, related
```

Navigation model: **persistent left rail** (categories) + **command palette**
(⌘K) for power users. Both must feel premium. Namespace and cluster are
global context selectors, always visible in the top bar.

---

## 7. Screen-by-screen

### 7.1 Top bar (global, persistent)
- Left: app mark "Kompass" + current **cluster** selector (with health dot).
- Center: **namespace** selector (multi-select / all).
- Right: global search hint (⌘K), connection/freshness indicator, theme toggle.
- Must communicate *which cluster am I touching* at all times — this is the
  #1 safety signal. Surfaced via the **auto-assigned per-cluster accent**
  (hashed from cluster name) on the cluster selector and a thin top-bar hairline.

### 7.2 Overview
- Calm dashboard: cluster name, k8s version, node count + health, workload
  rollup (running/pending/failed), recent warning events.
- Big readable numbers, sparklines, no chart clutter. Typography-led.

### 7.3 Resource list (the workhorse)
- Virtualized data table. Columns vary by kind. Sticky header.
- Per-row: name, namespace, status (semantic color + label), age, key metrics
  (e.g. ready 3/3, restarts), inline quick actions on hover.
- Powerful filter/search bar; saved filters later.
- Bulk select for batch ops.
- Empty, loading-cold, revalidating, and error states all designed.

### 7.4 Resource detail
- **Resizable side panel** (list stays visible left) with **fullscreen toggle**
  for deep YAML/logs/exec work. Drag handle to resize; collapse back to list+panel.
- Tabs: **Summary** (form view), **YAML** (editor), **Logs**, **Exec**,
  **Events**, **Related** (owner/owned, services→pods, etc.).
- Summary: labeled key/values, conditions, owner refs, clean grouping.
- YAML: monospace editor, diff-on-edit, validate, apply with confirm.
- Header actions: scale, restart, delete (guarded), edit.

### 7.5 Secrets (special care)
- Values **masked by default**; explicit reveal with friction (per-field).
- Decode/encode base64 inline. Edit with strong confirmation.
- Visual treatment that signals sensitivity (subtle, not alarming).

### 7.6 Logs
- Multi-pod tail: stack/merge streams, color per pod/container.
- Follow toggle, wrap toggle, search/highlight, timestamps, download.
- Must stay smooth under high log volume (virtualized, capped buffer).

### 7.7 Exec / terminal
- In-app terminal into a chosen container. Clear which pod/container/cluster.

### 7.8 Multi-cluster
- Fast switcher (in ⌘K and top bar). Recent/favorite clusters.
- Connection states per cluster: connected, connecting, unreachable, no-access.
- Later: side-by-side compare. v1: fast switch with clear active-cluster signal.

### 7.9 Command palette (⌘K)
- Navigate to any resource/kind/cluster; run actions; switch theme.
- Raycast-grade: fuzzy, fast, keyboard-only operable, beautiful.

---

## 8. Component inventory (for the design system)

- Top bar + cluster/namespace selectors (with health dots, accent)
- Left nav rail (collapsible, icon+label, active state)
- Command palette overlay
- Virtualized data table (sortable, selectable, hover actions, status cells)
- Status badge / pill (semantic states)
- Tabbed detail panel
- YAML/code editor surface (light+dark syntax themes)
- Log viewer (multi-stream, colored, virtualized)
- Terminal surface
- Form fields (key/value, labeled groups, masked secret field)
- Toasts / notifications (non-blocking)
- Confirmation dialogs (destructive-action guardrails)
- Freshness / connection indicator (the four data states)
- Empty / loading-cold / revalidating / error states
- Tooltip, dropdown, context menu

---

## 9. Visual system — what to define

Produce a **token set**, dark and light:

- **Color**: backgrounds (3–4 elevation layers), foreground text (3 emphasis
  levels), borders/dividers (whisper-thin), brand accent, semantic
  status (success/running, warning/pending, error/failed, info, neutral/unknown),
  **per-cluster accent palette: ~8 distinct, theme-safe hues** to hash cluster
  names into. WCAG AA contrast in both themes.
- **Typography**: UI sans (e.g. Inter/Geist class) + mono (e.g. Geist Mono /
  JetBrains Mono) for YAML/logs/terminal. Define a tight type scale and the
  hierarchy rules. Type does the heavy lifting.
- **Spacing**: a consistent base unit and scale; rhythm rules.
- **Radius, borders, shadows**: minimal, soft, consistent. Depth via subtle
  elevation/blur, not heavy shadows.
- **Motion**: durations + easing for nav transitions, palette open, state
  changes, freshness pulses. GPU-cheap (transform/opacity). Calm, never bouncy.
- **Density**: a comfortable default with a future "compact" mode in mind.

---

## 10. Design principles (decision rules)

1. **Type and space over lines and boxes.** Earn every border.
2. **Calm under live data.** Updates fade/settle, never flash or jump.
3. **Always show which cluster.** Safety through constant context.
4. **Cached-first, never block.** Instant from cache; revalidate quietly.
5. **Guardrails on destruction.** Delete/edit/secret-reveal need friction
   proportional to blast radius.
6. **Keyboard is first-class.** ⌘K and shortcuts for everything.
7. **Both themes, equally loved.**
8. **Progressive disclosure.** Simple front; depth one click deeper.

---

## 11. Resolved design decisions

- **Detail panel: resizable side panel + fullscreen toggle.** List stays visible
  left; detail slides in from the right; drag handle to resize; expand to full
  screen when deep in YAML/logs/exec. Collapse returns to list+panel.
- **Per-cluster accent: auto-assigned.** Hash cluster name → stable color from
  the cluster-accent palette. Zero setup, consistent across sessions/machines.
  Define a palette of ~8 distinct, theme-safe accents to hash into.
- **Density: comfortable only in v1.** Ship one hand-tuned density. Design tokens
  so a future "compact" mode is a clean addition, but don't design it yet.
- **Brand: compass motif.** Compass rose / needle mark paired with the "Kompass"
  wordmark. Ties to the name and the lens/radar lineage; navigation theme.
  Mark must read at 16px (tray/favicon) and scale up cleanly.

---

## 12. Deliverables requested from Claude Design

1. Token system (color/type/space/radius/shadow/motion), dark + light.
2. High-fidelity mockups: Overview, Resource list, Resource detail (Summary +
   YAML + Logs), Secrets, Command palette, Cluster switcher — in both themes.
3. Key states for the data table (cached/revalidating/cold/error/empty).
4. Component sheet covering §8.
5. Logo/wordmark direction.
