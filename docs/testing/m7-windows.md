# M7 Windows validation

- Milestone: M7
- First validation target: Windows x64
- Status: Passed with CI-assisted virtual Windows evidence

CI first launches the release executable on `windows-latest`, verifies the native
window title and 760×560 minimum-size contract, then uses the local WebView2
DevTools endpoint to assert that the DOM rendered the expected SonicMux content.
The production executable remains free of debugging flags in the
`sonicmux-m7-windows-manual` artifact. The separate
`sonicmux-m7-windows-validation` artifact contains the JSON report, WebView
screenshot, native-window screenshot, and process diagnostics. Review that
evidence before the interactive checks below.

The project owner accepted this native CI-assisted check as the M7 sign-off.
The production build and validation-only build are separate artifacts; remote
debugging is absent from the production Tauri configuration and executable.

## M7 evidence record

- Tester: Codex, with visual review of the captured WebView
- Environment: GitHub-hosted Microsoft Windows Server 2025 x64, build 26100
- WebView2: 150.0.4078.105
- Commit: `cda07ba13d86c44a5af5a7a24e937aea2c51f12e`
- Date: 2026-08-11
- CI run: <https://github.com/gl-epka/sonicmux/actions/runs/31466627050>
- Result: native window 760×560; WebView 744×501; DOM ready state `complete`;
  expected `SonicMux` and `FFmpeg` content present; reviewed screenshot has no
  horizontal overflow or clipped primary controls
- Validation screenshot SHA-256:
  `39d783f3e2a2fa28f2531ea800b9d6ccec3302dbb19fc19a672e2221235b0bf6`
- Production executable SHA-256:
  `ed73792724ca47a58aa96667d102a672f5959ed80167e9982488355bd322c96c`

The interactive checks below are intentionally retained for the M8 release
candidate, when SonicMux will have real redistributable FFmpeg payloads and
installers. They are not represented as having been performed by the CI smoke
test.

## Toolchain delivery

- [ ] With no configured, bundled, or `PATH` pair, the setup screen appears.
- [ ] Choosing `ffmpeg.exe` beside `ffprobe.exe` changes the status to explicit.
- [ ] Cancelling the native picker leaves setup recoverable.
- [ ] A system `PATH` pair is detected after **Retry system search**.
- [ ] A target-suffixed sidecar fixture wins over `PATH` in a packaged test.

## Queue and planning

- [ ] Native multi-file selection accepts `.mkv` files.
- [ ] Folder selection remains non-recursive and filters non-MKV files.
- [ ] Explorer drag/drop produces the same queue without exposing raw path IPC.
- [ ] Convert and remux settings rebuild the visible plan.
- [ ] Existing compatible audio is labelled as a skip/remux outcome.
- [ ] A probe error includes a working recovery action.

## Execution and cleanup

- [ ] A real conversion copies video and produces the planned audio.
- [ ] Aggregate and per-file progress update without freezing the window.
- [ ] **Cancel batch** reaps FFmpeg and removes private staging output.
- [ ] Closing during a batch retains the window until safe cancellation begins.
- [ ] No `ffmpeg.exe` or `ffprobe.exe` remains after completion or cancellation.
- [ ] An existing destination is not overwritten.

## Desktop behavior and accessibility

- [ ] The minimum 760×560 window remains usable with vertical compact layout.
- [ ] 100%, 150%, and 200% display scaling have no horizontal overflow.
- [ ] `Ctrl+O`, `Ctrl+Shift+O`, `Ctrl+Enter`, and `Delete` work.
- [ ] Keyboard focus is always visible and reaches every control.
- [ ] State remains understandable without color and under high contrast.
- [ ] Reduced-motion preference removes nonessential transitions.

## Extended interactive sign-off (M8 release candidate)

Tester: _pending for M8_

Environment: _pending for M8_

Commit: _pending for M8_

Date: _pending for M8_

Result: **Pending for M8; M7 CI-assisted validation passed**
