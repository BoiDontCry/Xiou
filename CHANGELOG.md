# Changelog

## [Unreleased]

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
- Current path-remapped isolated release is 10,956,800 bytes. Corrupt-index and real-index runs both opened responsive windows and exited with code 0; the corrupt-index log recorded only `NotADatabase(26)`, and the release binary scan found no build-user path or diagnostic test secrets.
- A 20-cycle real-input stability run repeatedly switched folder/message coordinates, edited/cleared search, and scrolled the reader; all 69 response samples passed and the process exited cleanly.
- Fresh 2026-08-19 rebuilds did not reproduce the earlier 65 MiB artifact baseline: the current build peaked near 71.6 MiB and a clean `e6ff0f2` rebuild peaked near 106.9 MiB under the same script. This is recorded as a build/runtime baseline discrepancy, not evidence of sustained growth; GPUI profiling remains out of R6/M4 scope.
