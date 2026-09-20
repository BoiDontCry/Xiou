# Changelog

## [Unreleased]

### GPUI migration E9 closeout — 2026-09-20 (unreleased)

- Removed the last migration remnant: the standalone `spike-gpui-kit` bin
  (`src/spike_gpui_kit.rs`) and its Cargo entry; no other old-widget code
  remains.
- Fixed a native-test regression introduced on 2026-09-18: the
  `overflow_x_scrollbar` wrapper rewrites the wrapped element's id, which
  detached the `mail-cid-image` test marker from the element tree and failed
  the E7 CID assertions. The marker id now lives on the stable outer image
  container; horizontal scrolling and render sizing are unchanged.
- Re-ran all four gates after cleanup: check/build pass, 111 unit tests and
  226 native GPUI records pass (same count as the 2026-09-14 baseline).
- Cargo.lock re-verified against `Cargo.toml` with `--locked --offline`.
- Measured an isolated E9 candidate (`target/e9-20260920/release/xiou.exe`,
  30.86 MiB) with 10,013 synthetic messages: startup peak working set
  85.38–89.58 MiB, 10-minute dwell stable with no leak trend; no budget
  numbers were invented. Real-OS interaction was accepted by manual use
  (`docs/GPUI-NATIVE-ACCEPTANCE.md`, `native-acceptance-20260920.md`).
- Not done in this batch: formal packaging and any budget-based acceptance;
  SMTP remains a fixed-config PoC.

### GPUI migration follow-up — 2026-09-14 (unreleased)

- Continued the E6/E7 implementation with E8 visual acceptance; this is not a
  completed native-OS acceptance or a published release.
- Fixed sidebar row overlap by placing ListItem content in an explicit flex row.
- Embedded the 12 additional mail SVGs required by the shared icon catalog while
  retaining official component assets; source-inventory tests cover all used icons.
- Fixed stale Component resolved tokens after theme changes, including buttons,
  dialogs, selection, progress and sliders; verify real composer button pixels.
- Added isolated E8 native geometry and four-theme/workspace/icon captures.
  Current verification: 108 unit tests and 226 native records passed; the exact
  83-PNG set was visually reviewed with remaining OS/DPI/image-quality gates explicit.
- Built an isolated 30.20 MiB release candidate without replacing the existing
  release; 3 index-only runs with 10,007 synthetic messages observed 69.19–69.23 MiB
  peak sampled working set. This is not a scrolling or lifetime-memory budget pass.
- Updated migration, usage and release documentation; SMTP remains a fixed-config
  PoC, and IMAP account setup does not yet imply generic multi-account sending.


### Added

- Added the GPUI desktop reader entry point and a real mailbox tree backed by the M2 SQLite index.
- Added grouped mailbox summaries with stable database IDs, deterministic ordering, and message counts.
- Added a read-only `MailStore` connection path for UI queries.
- Added paginated message summaries and a GPUI virtual list backed by the real SQLite index.
- Added HTML-to-text fallback rendering with blocked active content, comments, and entity decoding when a message has no usable plain-text body.
- Added regression tests for safe HTML fallback, blocked scripts/styles/iframes/resources, and plain-text precedence.
- Added an explicit body scroll handle so each newly selected message starts at the top while retaining native GPUI scrolling.
- Added a bounded `FOXMAIL_AUTO_EXIT_MS` measurement hook for repeatable startup, memory, and lifecycle checks without external window messages.
- Added structured missing, corrupt, cannot-open, and query-failure index diagnostics with actionable UI messages.
- Added append-only release logs containing event names and numeric SQLite/I/O fingerprints only; message content, credentials, full email addresses, paths, and raw database errors are excluded.
- Added portable path resolution: `FOXMAIL_INDEX_PATH`, then `FOXMAIL_BACKUP_DIR`, then an existing EXE-adjacent `data/mail-index.sqlite`, then `%LOCALAPPDATA%/foxmail-rs/data/mail-index.sqlite`.
- Added an explicit fallback when the log cannot be written because its parent is unavailable or unwritable.
- Added a reproducible Windows x64 ZIP packaging command with pinned Rust, MSVC, and Windows SDK versions, a path-remapped static-CRT release build, user instructions, and SHA-256 manifests.

### Changed

- Standard IMAP folder names are localized in the UI without changing their stored source names.
- The Windows GUI now opens centered without an accompanying console window.
- Switching folders now clears cached message pages and resets the virtual list to the top.
- The reader now prefers normalized plain text and treats HTML as inert display text; no HTML is passed to an executable WebView.
- Query errors now share one classification and logging path instead of exposing raw SQLite messages in each UI state.
- Startup/index failures show their full remediation text in the wide reader area; the sidebar keeps a compact status label, and search failures use a separate scrollable error block instead of the fixed status header.

### Fixed

- The reader now follows `FOXMAIL_BACKUP_DIR` when no explicit index path is configured.
- The mailbox tree hides obsolete UIDVALIDITY generations and remains deterministic for case-variant account and `INBOX` names.
- Long sender and subject text is truncated to preserve the virtual list's fixed row height.
- Responsive sidebar/list widths retain minimum reader space on high-DPI narrow windows.
- Added a read-only FTS5 search bar with keyboard editing, empty/error states, highlighted snippets, and mailbox-aware result navigation to message details.
- Search-result navigation now preserves the active query across mailboxes, while direct sidebar navigation exits search consistently.
- Search highlighting now maps case-insensitive matches back to UTF-8 boundaries instead of slicing with lowercased byte offsets.

### Verification

- Offline gates passed on 2026-08-19: `cargo fmt -- --check`, `cargo check --offline --all-targets`, `cargo test --offline --all-targets` (24 passed), `cargo clippy --offline --all-targets -- -D warnings`, debug GUI build, and release GUI build.
- Release binary before R5: 10,861,056 bytes; current isolated R6 build: 10,933,248 bytes.
- Three independent six-second real-index runs all opened responsive windows and exited with code 0. Peak working set averaged 66,356,565 bytes and reached 66,437,120 bytes; peak private bytes averaged 74,242,731 bytes and reached 74,330,112 bytes.
- A 60-second real-index smoke run produced 118 responsive samples, opened its window in about 539ms, and exited cleanly. Peak working set was 66,215,936 bytes, so the original 60 MiB memory target is not yet met.
- `git diff --check` and targeted secret scan passed. An initial all-target test run was blocked by a leftover `foxmail-rs` GUI process; after terminating that stale process, the complete gate passed normally.
- R6/M4 diagnostics gates passed with 33 tests: real invalid-SQLite classification, directory/metadata-open handling, missing/query-failure messages, path priority/fallbacks, unwritable-log fallback, log location, and a reserved-address/credential non-disclosure fixture.
- Current explicit-x64, path-remapped static-CRT release is 11,136,512 bytes. Its dependency table contains no `VCRUNTIME140.dll`, and the release binary scan found no build-user path or diagnostic test secrets.
- A 20-cycle real-input stability run repeatedly switched folder/message coordinates, edited/cleared search, and scrolled the reader; all 69 response samples passed and the process exited cleanly.
- Fresh 2026-08-19 rebuilds did not reproduce the earlier 65 MiB artifact baseline: the current build peaked near 71.6 MiB and a clean `e6ff0f2` rebuild peaked near 106.9 MiB under the same script. This is recorded as a build/runtime baseline discrepancy, not evidence of sustained growth; GPUI profiling remains out of R6/M4 scope.
- The final `foxmail-rs-0.1.0-windows-x64.zip` is 4,905,912 bytes with SHA-256 `B72C359EF60B87F2105BBCBE9C7EAF29A9614B92899D37CC1C7CAA22997CF569`; a deliberately polluted environment and a normal environment produced the same hash with the pinned local toolchain and controlled release profile.
- Clean extraction under a path containing spaces and Chinese characters passed user-data, portable-data, and unwritable-log scenarios. The packaged executable also passed 20/20 responsive auto-exit launches with no residual process.
