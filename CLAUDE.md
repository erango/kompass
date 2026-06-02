# CLAUDE.md

Guidance for working in this repo.

## What this is

Kompass — a Kubernetes management/visibility **desktop app** (Rust + Dioxus),
aiming for beautiful + fast (a Lens/Aptakube competitor). macOS-first.

## Architecture

Two-crate workspace (see `ARCHITECTURE.md`, `DESIGN_SPEC.md`):

- **`crates/kompass-core`** — UI-agnostic engine. Runs on its own background
  Tokio runtime. Connects, runs **discovery** (all built-in kinds + CRDs),
  `watch`es resources, and **normalizes** each object into a `ResourceRow`.
  - `model.rs` — data model + pure logic: `KindMeta`, `ResourceRow`,
    `Cmd` (UI→engine), `Delta` (engine→UI), per-kind status mappers +
    `normalize()`, `OverviewData`, container/age/event helpers. **Most unit
    tests live here.**
  - `watch.rs` — `run_engine` command loop, self-healing `watch_kind`
    (rebuilds client on error → refreshes expired exec creds), discovery,
    logs/exec (kubectl-PTY), metrics poller, overview fetch, port-forward,
    namespace-scope detection.
  - `lib.rs` — re-exports.
- **`crates/kompass-bin`** — Dioxus 0.7 desktop app.
  - `main.rs` — the whole UI (one big `App` + components). Talks to the engine
    via a global `CMD` sender (`send_cmd`) and consumes `Delta`s from a channel.
  - `config.rs` — persisted `Prefs` (JSON in the platform config dir).
  - `assets/` — CSS ported verbatim from the Claude Design bundles
    (tokens/app/detail/overlays/screens/containers/nav), injected via
    `style { dangerous_inner_html }`; `sprite.svg` icons; vendored `xterm/`;
    `icon/` (app icon + `gen-icon.sh` source).

**Engine↔UI contract:** UI sends `Cmd`, engine streams `Delta` over an unbounded
channel; UI renders from in-memory signals. The UI thread never blocks on I/O.

## Dev workflow

```sh
cargo build                 # build all
cargo test                  # run the suite (kompass-core + kompass-bin)
cargo run -p kompass-bin    # run against the current kube context

# macOS .app bundle (preferred for UI testing — exercises the real bundle):
./scripts/bundle.sh --debug --open      # debug build + launch
./scripts/bundle.sh                     # release, host arch
./scripts/bundle.sh --universal         # release universal (arm64 + x86_64)
```

- Uses the active kubeconfig context. Some clusters are real prod —
  **write actions hit live clusters** (pods delete immediately; everything else
  confirms first).
- Icons: edit `assets/icon/icon.svg`, then `./scripts/gen-icon.sh`.

## Conventions

- Match the surrounding code; CSS comes from the design system (oklch tokens,
  dual dark/light via `data-theme`, indigo accent).
- Dioxus patterns: `use_signal`/`use_callback`/`use_effect`/`use_hook`;
  `document::eval` for JS bridges (xterm, clipboard, keydown, scroll sync).
  Long-running search inputs use the debounced `SearchBox` component.
- Prefer adding **pure, testable** functions in `kompass-core` and covering them.
- Tooltips: the `.tip` + `data-tip` pattern; table-cell tooltips need the cell
  set to `overflow: visible` to escape it.

## Release strategy

Free, unsigned, macOS-first. **GitHub Releases + a Homebrew tap.**

- **Trigger:** push a `v*` tag → `.github/workflows/release.yml` runs on
  `macos-14`, builds a **universal** `.app` (`bundle.sh --universal`), packages a
  `.dmg`, and publishes a GitHub Release with the dmg + its sha256.
  ```sh
  # bump workspace version in Cargo.toml first, then:
  git tag v0.2.0 && git push origin v0.2.0
  ```
- **Homebrew:** a separate public tap repo `erango/homebrew-tap` holds
  `Casks/kompass.rb` (version + dmg url + sha256). Users:
  `brew install --cask erango/tap/kompass`. After each release, bump the cask's
  `version`/`url`/`sha256` (the sha256 is in the release notes / job summary).
- **Unsigned:** no Apple Developer ID, so Gatekeeper warns on first launch
  (right-click → Open, or `xattr -dr com.apple.quarantine`). Homebrew does **not**
  strip quarantine. To remove the warning later: enroll in Apple Developer
  ($99/yr) and add codesign + notarization (signing secrets) to the workflow.
- **Platforms:** macOS only for now. Linux (webkit2gtk + AppImage) and Windows
  are untested and deferred.
- **Screenshots for docs:** never use real clusters (leaks infra names). Spin up
  a throwaway `kind` cluster with its own kubeconfig + generic demo resources,
  point Kompass at it via `KUBECONFIG`, capture, then tear down.
