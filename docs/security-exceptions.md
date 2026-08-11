# Security exceptions

SonicMux treats dependency-advisory exceptions as temporary release blockers
with an explicit scope and removal condition. The weekly security workflow still
reports the ignored advisory identifiers, and maintainers review this file before
each release.

## RUSTSEC-2026-0194 and RUSTSEC-2026-0195

- Added: 2026-08-11
- Dependency path: `tauri` / `tauri-utils` → `plist` → `quick-xml 0.38.4`
- Affected behavior: pathological XML start tags can cause quadratic work or
  excessive namespace-allocation while using `quick-xml` namespace parsing.
- SonicMux exposure: SonicMux accepts MKV paths and delegates media parsing to
  FFprobe. It does not accept XML or plist input through its CLI, TUI, or GUI,
  and its content-security policy disables arbitrary remote content. The
  dependency is retained by the pinned Tauri packaging/runtime stack.
- Compensating control: release builds keep the GUI CSP test and dependency
  audit enabled; XML/plist is not introduced as an application input format.
- Removal condition: remove both ignores as soon as the stable Tauri dependency
  graph permits `quick-xml >= 0.41.0`.

These exceptions do not cover any direct SonicMux XML parser or any future
feature that accepts untrusted XML. Such a change must first remove or revisit
the exceptions.
