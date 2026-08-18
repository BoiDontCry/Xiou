# Changelog

## [Unreleased]

### Added

- Added the GPUI desktop reader entry point and a real mailbox tree backed by the M2 SQLite index.
- Added grouped mailbox summaries with stable database IDs, deterministic ordering, and message counts.
- Added a read-only `MailStore` connection path for UI queries.
- Added paginated message summaries and a GPUI virtual list backed by the real SQLite index.

### Changed

- Standard IMAP folder names are localized in the UI without changing their stored source names.
- The Windows GUI now opens centered without an accompanying console window.
- Switching folders now clears cached message pages and resets the virtual list to the top.

### Fixed

- The reader now follows `FOXMAIL_BACKUP_DIR` when no explicit index path is configured.
- The mailbox tree hides obsolete UIDVALIDITY generations and remains deterministic for case-variant account and `INBOX` names.
- Long sender and subject text is truncated to preserve the virtual list's fixed row height.
