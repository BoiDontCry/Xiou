# GPUI migration regression harness

Run the explicit native suite on Windows with desktop/GPU prerequisites:

```powershell
cargo test --locked --offline --features native-ui-tests --test gpui-migration
```

`native-ui-tests` enables the official `gpui-kit/test-support` feature (including
Windows DirectX offscreen capture and leak detection). Normal product builds and
plain unit tests do not enable this feature. Dependency versions remain locked;
offline commands require the corresponding Cargo cache to have been populated.

## Isolation and lifecycle

The custom harness runs on its own main thread. Before GPUI starts it creates a
fresh temporary profile, redirects account/preferences/index/backup paths,
removes the inherited IMAP password, and changes into that profile. Its EML
fixtures use `example.invalid`; no IMAP/SMTP transport is run. E5 delivers local
channels and send-result Tasks to the production business callbacks. Profiles,
reports and images are retained for inspection.

Every native window is non-visible and unfocused at the OS level. The harness
explicitly resizes hidden windows and checks the viewport: on Windows, deferred
initial placement can otherwise leave `CW_USEDEFAULT` dimensions and a 1x1
render texture. Degenerate captures fail. The earlier state-only fixtures must
not be cited as valid-sized screenshot evidence.

The test application uses explicit quit mode. It dismisses business popups,
drains deferred events/animations, removes its windows, releases entity handles,
then quits. Leak detection stays enabled. A printed PASS followed by a nonzero
process exit is a failure, not a successful run. The watchdog is 120 seconds to
cover the screenshot matrix and slow GPU initialization.

## Production paths covered

Production modules are included directly; helpers live in `tests/support/` so
Cargo does not accidentally discover them as standalone test targets.

- Root/assets initialization and rendered Dialog/Sheet/Notification services.
- Simulated onboarding Tab/Shift-Tab and existing composer input/draft behavior.
- Search clear/dismiss, real offline debounced queries, pending/empty/error states.
- Business Popover pointer selection, outside click, retained input focus,
  Clear button, recent/result navigation, Enter, Space and selected-item scrolling.
- Synthetic marked-text guards for search arrows/Enter/Esc. These do not certify
  a native Windows IME or candidate window.
- All editable color tokens and dark/light editing modes, silent programmatic
  synchronization, real Select keyboard and Slider pointer/keyboard callbacks,
  HEX validity, contrast validation, and hue retention through grey/black.
- Actual settings Dialog Tab trapping, save/cancel, Esc/backdrop dismissal,
  restoration of the invoking focus, isolated persistence and disk-failure drafts.
- Attachment business Dialog image/PDF fallback/error paths using offline EML,
  asynchronous cancellation/invalidation, and small-window layout bounds.
- Default/custom light/dark Kit/Base theme synchronization and already-open
  composer refresh after the actual settings-save path.
- E5 real sync event pump: busy/reentry, download versus indexing, actual offline
  import/reload, empty polls versus disconnected workers, stale generations/UIDs,
  resume offsets, silent no-change background polls, and status close/restore.
- Actual post-dialog attachment saves, cancelled targets, write errors, retries,
  message ownership across selection changes, and late/duplicate completions.
- Settings result notifications only after persistence; invalid HEX/cancel silence,
  retained drafts on disk failures, deduplication and recovery.
- Composer send snapshots and local completion Tasks: failure, retry, duplicate
  callbacks, edits during successful sends, another live window's newer draft,
  success-with-cleanup-warning, and completion after window destruction.
- Real Input Change autosave error/recovery and quiet normal editing. A Windows
  file handle that denies deletion exercises the sent-draft cleanup warning.
- Official business Toasts are observed as rendered elements, not just queued
  state. Channel replacement, independent results, dismiss and redraw are checked.

E5 helpers live in `support/gpui_feedback.rs` and `support/gpui_send_feedback.rs`.
They never call `attempt_send` with a populated recipient: valid fixture snapshots
use `begin_send` plus `dispatch_send` with an entirely local Task. The synthetic
Ctrl+Enter guard test keeps the recipient empty even if a guard regresses; the
four-field marked-text checks call the non-networking `begin_send` only.

The pinned Base Button documents an upstream missing `aria-disabled` setter.
Disabled Save is checked through real pointer behavior and business validation;
this is not a claim of completed native accessibility acceptance.

## Screenshots are a separate evidence category

`result.json` separates `state_event_layout_checks` from
`offscreen_screenshot_cases`. The matrix captures four themes across three
workspace layouts, settings, search, preview and an already-open composer.
PNG files are saved under the isolated profile's `screenshots/` directory.
Additional settings regression captures are kept there too.

These are native GPUI/DirectX offscreen renders, not OS-desktop captures or OS
keyboard events. `visual_review_complete` remains false until the images are
actually reviewed. Pixel dimensions/layout assertions and capture success do not
replace visual inspection, native IME, tray behavior, release memory/size or
large-list performance acceptance. See `docs/GPUI-MIGRATION.md` for the remaining
E6-E9 surfaces and overarching T4/T5 gates, including visual/OS review of E5.

The 2026-09-11 E5 final run passed 146 records: 103 state/event/layout checks and
43 screenshot cases. E5 adds 35 checks and 15 screenshot cases (50 records).
There are 45 PNG files including two earlier settings captures embedded in state
cases. E5 captures sync/send progress in four themes, indexing, attachment success
and failure, independent notifications, send failure and small-window cleanup /
autosave errors. `e5_checks` and per-record `batch: "E5"` support separate accounting.
All screenshots remain unreviewed: the image-view tool still fails to initialize
its Windows sandbox with error 1344. Capture and pixel checks do not waive review.

The required independent gates remain:

```powershell
cargo check --locked --offline --bin foxmail-rs
cargo test --locked --offline --lib --bin foxmail-rs
cargo test --locked --offline --features native-ui-tests --test gpui-migration
cargo build --locked --offline --bin foxmail-rs
```

The isolated `FOXMAIL_AUTO_EXIT_MS=3000` product smoke is separate from this
harness. Native test windows do not substitute for starting the product binary.


## 2026-09-14 E8: actual rendering, not just theme fields

The full current run passed **226 records**: 145 state/event/layout checks and
81 screenshot cases, with 83 PNGs including the two embedded settings captures.
E8 adds 24 geometry checks and 32 screenshot/cross-window cases. Run it alone:

```powershell
cargo test --locked --offline --features native-ui-tests --test gpui-migration -- --suite E8
```

`support/gpui_acceptance.rs` checks actual sidebar row/label/icon/count bounds
in expanded/collapsed and narrow-window layouts, using a long reserved-domain
account name. It renders all 23 used application icons through the same selective
AssetSource as the product. `ThemeProbe` also checks saved-primary-colored pixels
inside the real send button; legacy color fields alone missed stale resolved tokens.
The theme gate covers the Component render tokens as well as its Base projection.

The 83-image set has now been inspected using contact sheets and full-resolution
spot checks. The automatic `result.json` remains immutable and unreviewed-at-capture;
`visual-review.json` in the E8 evidence directory records image hashes and the later
review separately. Do not change the harness to always claim visual acceptance.
Native IME, OS focus/tray, multi-DPI and representative large images/attachments are
still separate gates; the existing tiny GIF fixture is not an image-quality test.
See `docs/GPUI-NATIVE-ACCEPTANCE.md`. The release candidate's index-only memory smoke
does not certify release scrolling or long-running performance.
