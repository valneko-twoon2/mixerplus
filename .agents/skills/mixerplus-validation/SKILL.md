---
name: mixerplus-validation
description: Validate mixerplus audio, tray UI, or harness changes before committing. Use for this repository's implementation tasks, not read-only discussion.
---

# mixerplus validation

Run `pwsh -File scripts/validate.ps1` from the repository root. It checks harness
mirrors, line endings, third-party notices, formatting, Clippy, and the small Rust test suite.

For UI or audio changes, additionally build with `cargo build --config src/.cargo/config.toml --manifest-path src/Cargo.toml --release --locked`
and run a focused Windows smoke check: no startup window, one tray icon, popup
open/dismiss, application rows, input/output menus, no-device behavior, and clean
exit. Check volume and mute only with explicit interaction; restore any changed
levels. Device-switch checks require the actual devices and must be reported
as unverified when unavailable. Do not automate an Explorer restart on the
user's desktop solely to test icon recovery.

For pagination changes, stop mixerplus and run the release binary with `--check-ui`.
This exercises native next/previous clicks and checks that page-one rows and
navigation HWNDs survive the round trip. For rendering changes, export previews
with `--snapshot src/target/preview.bmp --preview-dpi 96` and `--preview-dpi 144`.
These diagnostics only operate on mixerplus's own controls and never change audio.

Before completion, stage task changes, create an English Conventional Commit
(no `Co-Authored-By` trailer; the commit-msg hook rejects it), and run `pwsh -File scripts/complete-task.ps1`. Never commit pre-existing user
changes to make the completion check pass.
