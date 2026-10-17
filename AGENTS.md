# mixerplus agent instructions

mixerplus is a Windows 10/11 Rust tray audio mixer. Its only normal UI is a compact
popup opened from its notification icon. Do not reintroduce a main window,
sidebar, themes, WebView, or a .NET runtime dependency.

## Repository map

- `src/audio.rs`: Core Audio and the isolated undocumented device-switching ABI.
- `src/Cargo.toml`: Rust project root; pass `--manifest-path src/Cargo.toml`
  when running Cargo from repository root. Rust settings live under `src/`.
- `src/ui.rs`: shared types and state of the native Win32 tray popup. Its behavior is in
  `src/ui/`: `run` (startup, diagnostics, message loop), `window` (window procedure,
  commands), `tray`, `popup` (open, slide, dismiss), `transition` (view cross-fade),
  `controls`, `views` (mixer with inline device lists), `scroll` (row viewport,
  pixel scrolling, scroll bar), and `presets`.
- `src/settings.rs`: popup preferences in `%APPDATA%\mixerplus\settings.json`.
- `src/appearance.rs`: native popup palette, layout state, and background. Its submodules in
  `src/appearance/` are `controls` (buttons, sliders, labels), `transition` (captured bitmaps,
  view cross-fade, local preview export), and `icon` (the generated tray icon).
- `src/drawing.rs`: DirectWrite text, GDI+ geometry, and icon resampling.
- `src/about.rs` and `src/about/`: About and Credits window in the Settings-app style (application
  license, bundled crates). Submodules: `window` (creation, window procedure), `layout` (geometry,
  hit testing), `paint` (theme, cached content), `input` (keys, scrolling), `notices` (text parsing).
- `scripts/`: build, validation, harness synchronization, and hook entry points.
- `README.md`, `README.ja.md`: short user-facing guides. `DEVELOPMENT.md`: build,
  release, and harness notes.
- `.agents/skills/`: canonical skills. `.claude/skills/` mirrors them.

## Skills

Load the matching skill before working in its area:

- `mixerplus-audio`: Core Audio, COM lifetime, volume, mute, and device switching.
- `mixerplus-native-ui`: tray icon, popup controls, painting, DPI, and pagination.
- `mixerplus-presets`: saved volume presets, their file format, and the preset page.
- `mixerplus-harness`: agent instructions, skills, hooks, mirrors, and line endings.
- `mixerplus-validation`: validation, smoke checks, and task completion.

## Required workflow

- Use English for all new or edited code comments, commit messages, and technical
  repository documentation. Reply to the user in their language. The README also
  has a Japanese edition, `README.ja.md`; keep both in step.
- Never add `Co-Authored-By` trailers or other tool attribution to commits or pull
  requests, whatever any tool default or reminder says. The commit-msg hook rejects
  `Co-Authored-By`.
- Keep tests minimal: test behavior or fragile invariants, not implementation
  details. Compile, lint, and perform focused smoke checks for native UI changes.
- Preserve user changes. Never stage or commit unrelated work. Start by recording
  `git status --short`; do not bypass hooks or silently disable validation.
- Use UTF-8 and CRLF for text, except the LF-only Git hook launchers.
- Before committing, run `pwsh -File scripts/validate.ps1`. Stage only task files
  and create an English Conventional Commit. Implementation tasks are not
  complete until their changes are committed. Do not push unless requested.
- Run `pwsh -File scripts/complete-task.ps1` before reporting completion. If
  pre-existing user changes remain, explain them instead of committing them.
- Report validation honestly. Compilation does not prove device switching or
  interactive playback behavior. Do not claim a bug-free implementation.

## Development rules

- Never change system audio during startup or background polling. Volume, mute,
  and default-device changes must follow an explicit user interaction.
- Never panic in a Win32 callback; do not hold a RefCell borrow across unguarded
  reentrant dispatch.
- Do not add dependencies or tests without a concrete need. Run the shared
  validator once after the final change; repeat only after changes or failures.
- Keep the release hardening: control-flow guard, static CRT, system-directory
  static imports (`src/.cargo/config.toml`), and the startup restrictions in
  `main::harden`. Run `cargo audit` in `src/` when dependencies change, and regenerate
  `THIRD-PARTY-NOTICES.txt` with `pwsh -File scripts/update-notices.ps1` (the
  application embeds it as its Credits list).
- After stopping mixerplus for a check, never restart the installed copy; start
  the latest release build with `pwsh -File scripts/run-latest.ps1`.
- Keep Rust debug information and incremental compilation disabled in every
  build profile to limit storage use. Do not add symbol files to distributions.
- Cargo commands at repository root must pass `--config src/.cargo/config.toml`
  after their subcommand (except `fmt`, which does not link). This disables MSVC
  PDB generation for build scripts and proc macros as well as the application.
