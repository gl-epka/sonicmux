# M7 design: Tauri desktop interface

- Status: Accepted
- Date: 2026-08-11

## Scope and outcome

M7 replaces the provisional GUI crate with a working Tauri 2 desktop
application for Windows, Linux, and macOS. It is a thin adapter over the same
configuration, discovery, probing, pure planner, safe execution, and bounded
M5 scheduler used by the CLI and TUI.

The first manual validation target is Windows. CI must build the application on
all three operating systems. M7 implements the bundled-sidecar contract and the
system fallback, but does not publish redistributable FFmpeg payloads. Pinned
LGPL binaries, checksums, corresponding source, notices, signing, and release
artifacts remain M8 work under ADR-0002.

M7 does not add automatic FFmpeg downloads or updates, in-place replacement,
overwrite, remote content, an updater, or release signing.

## Technology choices

- Tauri 2.11.x with the system webview and the workspace MSRV of Rust 1.85.
- Svelte 5, Vite, and TypeScript as a static single-page frontend.
- npm with a committed lockfile.
- Hand-authored semantic CSS and design tokens; no CSS framework.
- One consistent outline SVG icon family; no emoji controls or remote assets.
- Vitest and Testing Library for frontend behavior.

The application is offline-first. It bundles no web font and uses the native
system UI font stack. This is intentionally more conservative than the search
tool's Inter recommendation: it removes a remote request and follows the host
desktop more closely.

## Product layout

The application has one main window with a preferred size near 1180 by 760 and
a usable compact layout below 1024 pixels:

1. A top bar shows the SonicMux identity, selected device profile, and FFmpeg
   source/status.
2. The primary pane contains the file queue, per-file state, planned action,
   progress, and ETA.
3. A detail pane shows tracks and session settings. At compact widths it
   becomes labelled `Tracks`, `Settings`, and `Logs` tabs.
4. A persistent action bar shows aggregate progress and exposes Start or
   Cancel, never both as ambiguous primary actions.
5. The empty queue is an actionable drop zone with native file and directory
   picker buttons.

The queue is windowed for large batches. Selection, keyboard focus, and the
full item count remain visible. Filename display is lossy-only presentation;
the frontend never round-trips an operating-system path.

## Visual system

The default appearance is a restrained dark media utility, with a system light
variant:

- deep slate surfaces rather than pure black;
- cyan as the primary interactive accent, matching the TUI;
- green, amber, and red reserved for textual success, warning, and failure
  states;
- a 4/8 pixel spacing rhythm and a compact desktop density;
- tabular numerals for progress and ETA;
- 150–220 ms opacity/transform transitions only;
- all motion disabled or reduced under `prefers-reduced-motion`.

Text and state pairs target WCAG AA. Color is never the only state indicator.
Every interactive control has a visible focus ring, accessible name, hover,
pressed, disabled, and busy state.

## Interaction model

Primary shortcuts are:

| Shortcut | Action |
| --- | --- |
| `Ctrl/Cmd+O` | Add MKV files |
| `Ctrl/Cmd+Shift+O` | Add a directory |
| `Ctrl/Cmd+Enter` | Start the enabled ready queue |
| `Delete` | Remove the selected idle item |
| `Escape` | Close a dialog or retain an active batch |

Native menu items expose the same actions. Drag and drop is an enhancement,
not the only input mechanism. Batch-active settings and destructive queue
operations are visibly and semantically disabled until cleanup finishes.

Errors appear next to the affected file or field and always name a recovery
action. A fatal background error is a persistent alert. Closing the window
during a batch is intercepted: the user confirms cancellation, the runtime
waits for process and staging cleanup, and only then does the window close.

## Trusted path grants

The frontend cannot invoke a command with an arbitrary path. Native dialogs
and operating-system drag/drop are handled in Rust. Rust records selected roots
inside the GUI session and exposes only stable item identifiers and lossy
display strings.

This provides four properties:

1. non-UTF-8 paths remain lossless inside Rust;
2. the webview cannot forge a path to an unrelated local file;
3. recursive discovery remains constrained to user-selected roots;
4. output-directory selection is a separate explicit grant.

Configured paths remain trusted local configuration inputs. Symbolic-link
following stays disabled unless the effective configuration explicitly enables
it in a future milestone.

## Rust application boundary

Tauri commands delegate to a framework-neutral `GuiService`. The service owns
the GUI session model and composes `sonicmux-runtime` and `sonicmux-ffmpeg`.
Tauri-specific command functions contain serialization and state extraction
only.

Conceptual contract:

```text
bootstrap(channel) -> BootstrapDto
pick_inputs(kind) -> SessionSnapshotDto
pick_output_directory() -> SessionSnapshotDto
remove_items(ids) -> SessionSnapshotDto
set_item_enabled(id, enabled) -> SessionSnapshotDto
update_settings(settings) -> SessionSnapshotDto
start_batch() -> AcceptedDto
cancel_batch() -> AcceptedDto
retry_items(ids) -> SessionSnapshotDto
choose_ffmpeg() -> ToolchainStatusDto
```

`BootstrapDto`, `SessionSnapshotDto`, `QueueItemDto`, `TrackDto`,
`SettingsDto`, `ToolchainStatusDto`, and `GuiEventDto` are explicit frontend
contracts. They do not serialize domain types wholesale. DTO fields use
`camelCase`; tagged event variants are versioned and bounded.

## Progress and state synchronization

One ordered Tauri IPC channel is registered by the main window during
bootstrap. The Rust adapter consumes M5 `watch` snapshots and bounded
`broadcast` lifecycle events, then sends coalesced GUI events. High-frequency
progress is limited to a UI-suitable cadence. Re-bootstrap replaces a stale
channel and returns the authoritative current snapshot.

Commands mutate one serialized session state machine:

```text
toolchain-setup -> idle/probing -> ready -> running -> cancelling -> idle
```

The scheduler report remains the source of truth for terminal file outcomes.
The frontend never predicts a successful execution.

## FFmpeg resolution and sidecar contract

GUI resolution order is:

1. a path explicitly selected for this GUI session;
2. environment or effective TOML configuration;
3. an installed sidecar `ffmpeg` and `ffprobe` pair;
4. the system `PATH`;
5. the first-run setup state.

The sidecar resolver is a Rust API that accepts an optional installed bundle
directory and returns both executable paths plus a source label. Tauri release
configuration declares target-suffixed `externalBin` entries. CI uses executable
test fixtures to audit naming and resolver behavior; it does not present them
as redistributable FFmpeg.

The setup screen provides three explicit actions: choose an existing FFmpeg
installation, retry system discovery, or view platform installation guidance.
The application never silently downloads or executes an unverified payload.

## Tauri security boundary

- One local `main` webview and no remote capability origins.
- A strict content security policy with no remote scripts, styles, frames, or
  network destinations.
- An explicit capability identifier enabled in `tauri.conf.json`.
- Only the native open dialog and minimum required core window/event APIs.
- No frontend filesystem, shell, opener, HTTP, or process permission.
- Custom commands restricted through the Tauri application manifest.
- FFmpeg child processes launched only by the existing Rust backend.

Capability tests parse every configuration file and fail on wildcards,
`allow-all`, remote URLs, filesystem permissions, or shell permissions.

## Testing and delivery

Rust tests cover command DTO serialization, session transitions, path grants,
toolchain precedence, scheduler reconciliation, cancellation, stale channels,
and errors. Mock backends keep command-contract tests deterministic.

Frontend tests cover the empty state, queue selection, settings validation,
disabled active-batch controls, progress announcements, retry paths, keyboard
shortcuts, compact layout semantics, and setup screen. Accessibility tests
check semantic labels and common automated violations.

The GUI CI job runs frontend install/typecheck/test/build and a real Tauri
application build on Windows, Ubuntu, and macOS. Linux installs the documented
WebKitGTK build prerequisites. The ordinary workspace checks, MSRV, and real
FFmpeg integration stay mandatory.

Documentation includes:

- GUI development and build commands;
- capability and sidecar audit instructions;
- limitations and FFmpeg setup behavior;
- deterministic screenshots of empty, planned, running, error, and setup
  states;
- a Windows manual checklist covering sidecar, system fallback, cancellation,
  resize, keyboard use, and terminal process cleanup.

M7 is complete only after the three-platform build is green and the Windows
manual checklist is explicitly signed off.

## Accepted approval points

Approval accepts:

1. Svelte 5 plus Vite and TypeScript without SvelteKit or a CSS framework;
2. ordered Tauri channels for progress instead of high-frequency `app.emit`;
3. Rust-owned path grants and identifiers rather than raw frontend paths;
4. a dark-first single-window dashboard with compact tabs;
5. sidecar-aware packaging and resolution in M7, with real redistributable
   FFmpeg payload publication deferred to M8;
6. Windows as the first manual validation target.
