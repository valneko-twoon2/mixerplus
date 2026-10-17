---
name: mixerplus-native-ui
description: Rules for mixerplus's Win32 tray icon and popup - child controls, custom painting, text rendering, DPI, keyboard handling, and pagination in src/ui.rs, src/appearance.rs, and src/drawing.rs.
---

# mixerplus native UI

## Popup behavior

- Keep the popup within the active monitor work area and support DPI, keyboard
  navigation, Escape, outside-click dismissal, and Explorer restart recovery.
- Defer row rebuilds until native click notifications unwind; never recycle the
  clicked button's HWND as a different control. Exercise `--check-ui` after mixer
  scrolling or device list edits.
- The popup refreshes every second while visible. Update captions with
  `set_text`, which skips unchanged text, so polling does not repaint controls.
- `clear_rows` hides row controls into `App::spare` and `child` reuses them, since
  creating and destroying windows dominated view switches. Reset per-use state in
  `appearance::recycle`; the clicked button and EDIT boxes are destroyed instead.
- Show and hide child controls with `ShowWindow`, not `WS_VISIBLE` at creation or
  `SWP_SHOWWINDOW`/`SWP_HIDEWINDOW`, which take a path about a hundred times slower.
  Showing many controls in a visible popup goes inside `WM_SETREDRAW` off/on.

## Layout and motion

- The popup imitates the Windows 11 quick settings audio flyout. Keep that look:
  no gradients or custom themes beyond the system light/dark palette and accent
  in `appearance.rs`. The volume value beside each slider is a deliberate mixerplus
  addition.
- Views: `View::Presets` (see `mixerplus-presets`) and `View::Mixer`: an output and an
  input group, then the section header and application rows. A group is a title with
  a chevron (`OUTPUT_CHEVRON`, `INPUT_CHEVRON`) and device rows: an item that makes the
  device the default and the device's own volume (no per-device mute button). The
  heading's mute button (`OUTPUT_MUTE`, `INPUT_MUTE`, `GroupMute`) mutes the default
  device; its glyph follows the level and mute state and is refreshed with the rows. Collapsed, a
  group shows only its default device; expanded, every device, and the output group
  ends with its spatial sound formats (立体音響). The input group starts below a
  divider. The expanded state is remembered in `settings.json` (`settings.rs`). View
  changes and chevron toggles are posted as `REFRESH_PAGE` so the clicked button's
  notification unwinds first.
- Win+Alt+V (`RegisterHotKey`, `HOTKEY`) toggles the popup like a tray click; it only
  opens or closes, and never changes the saved expanded state. A header pinned above
  the viewport (title 音声出力 and key caps; `KEYS` button kind, caption `Win+Alt+V`,
  drawn by `draw_item`, no Tab stop) stays while rows scroll, like the shell's output
  page. It exists only on the mixer and only when registration succeeded, since
  another application may own the shortcut; a failure is not reported. The viewport
  starts below it (`App::header`), so dividers are viewport-relative and the popup
  paints them offset by `Layout::top`.
- Row controls are children of the viewport (`ui/scroll.rs`), a control-parent child
  window at the popup's client origin that clips them and forwards their notifications
  to the popup; the footer link stays a direct child. The mixer scrolls by pixels with
  `ScrollWindowEx` and never rebuilds to scroll (about 1 ms per step). Wheel and page
  scrolls ease out over time; a thread waits on `DwmFlush` and posts `SCROLL_FRAME`
  once per display refresh, since 15.6 ms timer ticks out of step with the display
  stuttered. Sliders pass the wheel to the popup, so it scrolls instead of changing volume,
  as in the shell flyout. After Tab, `TAB_KEY` scrolls the focused row into view.
  Captures clip viewport children to it, and the viewport paints the dividers and the
  text box frame like the popup.
- The scroll bar is painted by the popup in the strip right of the viewport. The popup
  handles its mouse messages: drag the thumb (with capture), press the track to page,
  and hover to widen it.
- Owner-drawn buttons pack their kind, Fluent glyph, and selection/mute flag in
  `GWLP_USERDATA` (`appearance::pack`). Use `set_flag` for updates.
- Sliders show a live peak meter (`IAudioMeterInformation`, polled every 33 ms
  while the popup is visible and stored in a window property). Keep polling cheap:
  repaint only when the quantized value changes.
- Switching views cross-fades captured bitmaps. While it runs the controls are
  hidden and the popup paints the transition itself (`appearance::compose`);
  `finish_layout` defers the window resize to `begin_transition`, so the window
  never shows the new size early. Never reintroduce a child overlay: lower
  siblings repaint over it.
- The popup sits at the bottom-right of the work area with a 12 px margin. It is
  inserted just below the taskbar in z-order (again after activation) and slides
  from behind it with ease-out, then back with ease-in; animations follow the
  Windows client-area animation setting. A tray click within 400 ms of a
  dismissal toggles the popup closed instead of reopening it.

## Painting

- Labels use `WM_CTLCOLORSTATIC` with a null brush and a subclass that paints
  the surface. STATIC controls draw synchronously inside `WM_SETTEXT` without
  `WM_PAINT`; `label_proc` suppresses that drawing and invalidates instead.
  Removing it makes refreshed labels stack ghosted text.
- Set fonts without redraw and invalidate after installing a paint subclass;
  otherwise a control created in a visible popup validates itself unpainted.
- Text uses DirectWrite layouts drawn through its GDI-compatible bitmap render
  target with grayscale antialiasing; text formats are cached. A Direct2D DC
  render target cost about 0.15 ms of binding and flushing per string. The face,
  weight, and size come from the DC's selected font: `Segoe UI Variable Text`
  falls back to Yu Gothic UI for Japanese like the shell, whereas GDI font
  linking would pick Meiryo UI.
- Every custom paint goes through an offscreen `Frame` to avoid flicker. Shapes
  and icons use GDI+. Flush GDI before GDI+ or DirectWrite draws on a DC.

## About window

- `about.rs` (with `about/`) is one custom-painted top-level window laid out like a Settings page
  (large title, cards on a flat background; no icon or footer). The tray menu opens
  it on the license (*mixerplus について*) or on the Credits list (*Credits*); a
  crate's license is a third view with a back button. It is owned by a hidden `STATIC`
  window so it has no taskbar button; keep that owner when changing creation.
- Its scrolling content is rendered once into an offscreen bitmap (`Cache`) and
  only copied while scrolling; anything that changes the content must be part of
  `CacheKey`. The caption text and icon are hidden with `SetWindowThemeAttribute`.
- The executable's version resource (`build.rs`) names the product `mixerplus`, the
  same as the binary; keep `FileDescription`, `ProductName`, and `InternalName` equal.
- The popup has an empty window title (except with `--inspect-ui`) so Task Manager
  does not list it as an app window.

## Tray icon

- The icon is generated at runtime (`appearance::tray_icon`): the taskbar speaker
  glyph in the theme accent color. Its coverage is rendered over the matching
  taskbar shade before becoming alpha, so weight matches the system icons. It is regenerated on
  theme changes. Left-click toggles the popup; right-click shows the tray menu
  (`tray_menu`), which hides the popup first because the menu runs its own
  message loop and would stall a sliding popup.
- Device list selection changes update item flags in place (`REFRESH_STATE`);
  only list changes rebuild, and visible rebuilds run inside `quietly`, which
  suspends redraw so view switches do not flicker.

- `maintain_tray` retries adding the icon until Explorer accepts it, probing with
  `NIM_MODIFY` first because `NIM_ADD` can time out after succeeding.
- On Windows 11 it promotes the icon next to the system icons by setting
  `IsPromoted=1` under `HKCU\Control Panel\NotifyIconSettings\<id>` for the entry
  whose `ExecutablePath` matches this executable. It only writes when the value
  is absent, so a later choice in Windows Settings is preserved.

## Checks

For rendering changes, export previews with
`--snapshot src/target/preview.bmp --preview-dpi 96` and `--preview-dpi 144`, and
watch a live popup for a few seconds so refresh-driven repaints are covered.
