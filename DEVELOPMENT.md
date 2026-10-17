# mixerplus development

Notes for people building or changing mixerplus. For installing and using it, see [README.md](README.md).

## Build and run

Install the stable Rust MSVC toolchain and Visual Studio C++ Build Tools, then:

```powershell
pwsh -File scripts/setup-harness.ps1
cargo run --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked
# Open the popup immediately, including when mixerplus is already running:
cargo run --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked -- --show
# Read-only audio enumeration without starting a tray loop:
cargo run --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked -- --check-audio
# Expose the popup to desktop inspection tools during development:
cargo run --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked -- --show --inspect-ui
# Render only mixerplus's own controls to a BMP and exit:
cargo run --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked -- --snapshot src/target/preview.bmp
# Read-only native next/previous regression check (exit running mixerplus first):
cargo run --config src/.cargo/config.toml --manifest-path src/Cargo.toml --locked -- --check-ui
# Build the standalone release:
pwsh -File scripts/publish.ps1
```

`scripts/build.cmd` builds and starts the development executable.
`scripts/release.cmd` creates the release in `artifacts/`. All Cargo profiles
explicitly disable debug information and incremental compilation to limit disk
usage. Release files contain no PDBs and have no .NET runtime dependency.

## Releasing

`scripts/set-version.cmd [X.Y.Z]` sets the version in `Cargo.toml`, `Cargo.lock`, and
`app.manifest` (it asks when no version is given). Commit the result before publishing.

`scripts/publish.ps1` fills `artifacts/` with exactly the release assets (removing anything else):
`mixerplus.exe`, `mixerplus.exe.sha256`, `install.ps1`, and `uninstall.ps1`.
Attach everything in `artifacts/` to the GitHub release; the one-line installer
downloads these assets from `releases/latest/download`. `install.ps1` has a
`-Repo` parameter (default `valneko-twoon2/mixerplus`); update it and the README URLs if the
repository is renamed. The installer registers the sign-in start (`Run` key) and a
Settings > Apps entry; `uninstall.ps1` removes both. Because the install path is
fixed, the tray icon promotion applies to every update.

## Source layout

- `src/config.rs`: application configuration defaults.
- `src/settings.rs`: popup preferences (which device lists are expanded). mixerplus
  writes only `presets.json` and `settings.json` in `%AppData%\mixerplus`; it never
  records the microphone.
- `src/audio.rs`: endpoint/session enumeration, volume, mute, and default devices.
- `src/ui.rs` and `src/ui/`: native tray icon and popup. No main-window UI is included.
- `src/appearance.rs` and `src/appearance/`: native rendering and application-only preview export.
- `src/drawing.rs`: antialiased text, shapes, and high-quality icon scaling.
- `src/app.ico`: the application icon, embedded by `src/build.rs` as a resource
  without a resource compiler. Regenerate it with `pwsh -File scripts/make-icon.ps1`
  (needs the Segoe Fluent Icons font of Windows 11).

The old WPF projects and their local build artifacts are removed. Their history
is available in Git. Themes are not carried over. Presets are: the file format is
the old version's, so existing presets still load. Start at sign-in is set up by
the installer.

## Audio behavior and limitations

Volume and mute use supported Core Audio APIs. Only applications with an audio
session appear; exclusive-mode playback can bypass session volume controls.
Multiple sessions from one executable share a row across devices; changing that row
sets all its sessions to the same value. The displayed value comes from its
first session. A browser's tabs are not guaranteed to have separate controls.

Default-device switching uses the isolated, undocumented Windows PolicyConfig
COM interface. It changes console, multimedia, and communications defaults.
Windows updates or application-specific device selection can affect switching;
applications pinned to another device may not follow the new default. Failure
is reported, and the popup can be reopened to refresh the actual state. A failed
multi-role switch may have changed an earlier role before failing. Per-app
output routing and vendor-specific enhancements are not implemented. More links
to Windows sound settings for settings outside this popup.

Devices and sessions refresh once per second only while the popup is open;
refresh pauses during slider dragging. Missing devices are normal. The tray icon
is recreated after Explorer restarts. Only one instance runs per login session.

## Agent harness

Root `AGENTS.md` holds the shared instructions; there is no `CLAUDE.md`. Area
rules live in skills: `mixerplus-audio`, `mixerplus-native-ui`, `mixerplus-presets`,
`mixerplus-harness`, and `mixerplus-validation`. `.agents/skills/` is canonical; `.claude/skills/` mirrors it.
`.codex/` contains only config and hooks. There are no `.agents/rules`, `.codex/rules`, or `.codex/skills` directories. Mirrors follow
each tool's supported discovery paths, rather than duplicating directory trees.
Edit the canonical files and run `pwsh -File scripts/sync-harness.ps1`.
`-Check` detects drift.
The Rust project root is `src/`: Cargo.toml, Cargo.lock, rustfmt.toml, build.rs,
and the application manifest live there alongside the Rust source. Cargo
commands at repository root pass `--manifest-path src/Cargo.toml`; build output
is under `src/target/`. Git attributes and agent settings remain at their
required repository locations.

The shared validator runs mirror, CRLF, and third-party notice checks, rustfmt, Clippy with warnings
as errors, and two layout tests. New comments, documentation, and commits use
English. Tests are deliberately small; actual hardware behavior needs a focused
Windows smoke check.

```powershell
pwsh -File scripts/check-line-endings.ps1 -Fix
pwsh -File scripts/validate.ps1
# Stage only the current task's changes, then create an English Conventional Commit.
git commit -m "feat: describe the completed change"
pwsh -File scripts/complete-task.ps1
```

`THIRD-PARTY-NOTICES.txt` lists the crates in the release executable and is shown
in the app (tray menu > Credits). After changing dependencies run
`pwsh -File scripts/update-notices.ps1`; the validator fails while it is stale.

Setup installs repository-local Git hooks. Pre-commit rejects invalid line
endings, mirror drift, build/lint failures, and mixed staged/unstaged tracked
edits; commit-msg requires an ASCII Conventional Commit without a `Co-Authored-By`
trailer. Attributes enforce
CRLF working-tree text and normalized LF Git blobs. The tiny POSIX Git hook
launchers use LF so Git for Windows can execute them.

Codex and Claude Stop hooks share `scripts/agent-stop.ps1`. They request a
continuation when uncommitted changes remain, with a retry guard to avoid an
endless loop over pre-existing user work. Codex requires project and hook trust:
review the generated hook using `/hooks` before relying on it. Existing sessions
may need to restart to load new project configuration. See the official
[Codex hooks](https://learn.chatgpt.com/docs/hooks) and
[Claude hooks](https://code.claude.com/docs/en/hooks) documentation.

Local hooks can be bypassed and do not enforce server policy. They never create
a commit automatically or stage unrelated changes. A completed implementation
task must include its own commit; read-only discussions need none.
