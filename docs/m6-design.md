# M6 terminal interface design

- Status: Accepted
- Date: 2026-08-11

## Scope

M6 replaces the `sonicmux-tui` placeholder with a keyboard-first terminal
dashboard over the existing runtime. The TUI owns presentation and interaction;
media probing, pure planning, safe execution, progress aggregation, and
cancellation remain in the shared application layers.

The first version supports an editable queue, selected-file track details,
session settings, a bounded event log, aggregate and per-file progress, retry,
and orderly cancellation. It accepts optional MKV files, directories, and glob
patterns at startup and can add more paths from inside the application.

## Dependency decision

The workspace keeps MSRV 1.85. Ratatui 0.30.2 requires Rust 1.88, so M6 pins
Ratatui 0.29.0, whose declared MSRV is 1.74. Version 0.29 already provides the
cross-platform Crossterm backend, alternate-screen initialization, restoration,
and a panic hook that restores the terminal. No unstable Ratatui feature is
enabled.

## Information architecture

The interface has four stable screens:

1. Queue: discovered files, lifecycle state, progress, and selected-file
   summary on wide terminals.
2. Tracks: streams and the planned action for the selected file.
3. Logs: a bounded ring of lifecycle and failure messages.
4. Settings: profile, target audio, output mode, concurrency, storage profile,
   failure policy, and dry-run state.

The layout is responsive rather than horizontally scrollable. At 110 columns
the queue and details share the main area. Between 76 and 109 columns details
move to their own screen. Below 76 columns optional table columns disappear. A
terminal below 50 by 12 cells renders only a clear minimum-size message.

Color supplements, but never replaces, status text. Stable labels such as
`[RUN]`, `[OK]`, `[ERR]`, `[WAIT]`, and `[CXL]` remain meaningful with
`NO_COLOR`. The TUI does not enable mouse capture in M6.

## Interaction

Navigation supports both arrow keys and Vim conventions. `1` through `4` select
screens, `Tab` and `BackTab` cycle them, `j/k/g/G` move the queue selection,
`a` opens the path editor, `d` removes an idle item, Space toggles whether an
item participates, `s` starts, `c` or Ctrl+C cancels, `r` re-enables failed or
cancelled items, `?` opens help, and `q` exits when idle. Quitting an active
batch asks for cancellation and waits for runtime cleanup.

Settings are session-local. Profile or audio changes rebuild plans from cached
`MediaInfo` through the pure planner. They do not rewrite the TOML file.
Settings and destructive queue edits are locked while a batch is running or
cancelling.

## TEA model

```rust,ignore
struct Model {
    screen: Screen,
    phase: AppPhase,
    queue: Vec<QueueItem>,
    selected: Option<QueueId>,
    settings: UiSettings,
    snapshot: Option<Arc<BatchSnapshot>>,
    overlay: Option<Overlay>,
    logs: VecDeque<LogEntry>,
    dirty: bool,
}

enum Msg {
    Input(KeyEvent),
    Resize(u16, u16),
    Tick,
    InputsDiscovered(Result<Vec<PathBuf>, DiscoveryError>),
    ProbeFinished(QueueId, Result<MediaInfo, RuntimeError>),
    BatchSnapshot(Arc<BatchSnapshot>),
    BatchEvent(BatchEvent),
    BatchFinished(Result<BatchReport, SchedulerError>),
}

enum Effect {
    Discover(DiscoveryRequest),
    Probe(QueueId),
    StartBatch,
    CancelBatch,
    Quit,
}
```

`update(Model, Msg) -> Vec<Effect>` and `view(Frame, &Model)` perform no I/O.
The application loop interprets effects. Crossterm input is read on one
dedicated thread with a bounded poll interval; discovery, probe, and scheduler
work run as Tokio tasks. Rendering is dirty-driven and capped at 30 frames per
second.

Runtime `watch` snapshots are authoritative and recover the UI after broadcast
lag. Broadcast events feed the log. A new frontend-neutral failure event may
carry the failure stage and bounded diagnostic without changing CLI JSON output.

## Queue lifecycle

New roots are discovered by `sonicmux_runtime::discover`. Each unique file gets
a stable TUI queue identifier and is probed in a bounded background task so the
track screen is useful before execution. Starting a batch deliberately lets the
M5 scheduler probe again: the second probe is cheap compared with container I/O
and prevents execution from trusting stale preflight data.

The active batch stores an explicit mapping from scheduler `JobId` values to TUI
queue identifiers. Completion applies the authoritative `BatchReport`, retains
failure diagnostics, and makes failed or cancelled items retryable.

## Terminal lifecycle

`color-eyre` and tracing are installed before Ratatui so Ratatui can wrap the
existing panic hook. TUI tracing never writes to the active alternate screen;
it uses the configured structured log file. Normal exit stops and joins the
input thread before calling `try_restore`. Ratatui's panic hook is the panic-path
fallback.

## Verification

- Pure update tests cover navigation, selection bounds, overlays, edit locks,
  cancellation, retry, and state reconciliation.
- Ratatui `TestBackend` snapshots cover wide, medium, compact, empty, running,
  failed, settings, help, and too-small layouts.
- Mock-runtime tests cover discovery/probe delivery, snapshot coalescing,
  broadcast lag, cancellation, and final report mapping.
- A lifecycle harness verifies restoration during unwinding, supplemented by a
  real-terminal smoke test.
- A generated FFmpeg fixture exercises an actual dry run and conversion.
- A deterministic VHS tape produces the README demonstration GIF.
- Formatting, Clippy with warnings denied, tests, rustdoc, MSRV 1.85, and the
  three-OS CI matrix must all pass.
