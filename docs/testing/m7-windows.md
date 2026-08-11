# M7 Windows manual checklist

- Milestone: M7
- First manual target: Windows 11 x64
- Status: Pending a physical or virtual Windows run

Record the tester, Windows build, WebView2 version, FFmpeg build, commit, date,
and evidence links before changing the status to `Passed`.

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

## Sign-off

Tester: _pending_

Environment: _pending_

Commit: _pending_

Date: _pending_

Result: **Pending**
