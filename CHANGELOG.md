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

### Changed

- Standard IMAP folder names are localized in the UI without changing their stored source names.
- The Windows GUI now opens centered without an accompanying console window.
- Switching folders now clears cached message pages and resets the virtual list to the top.
- The reader now prefers normalized plain text and treats HTML as inert display text; no HTML is passed to an executable WebView.

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
