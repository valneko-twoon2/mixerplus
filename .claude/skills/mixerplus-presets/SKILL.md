---
name: mixerplus-presets
description: Rules for MixerPlus volume presets - the presets.json format shared with the old .NET version, global scope, favorite labels, applying and capturing levels, and the preset page in the popup.
---

# MixerPlus presets

## Data

- Presets live in `%AppData%\MixerPlus\presets.json`, a JSON array written by
  `src/presets.rs` through the small bounded parser in `src/json.rs`. No serde or
  other dependency is used; keep it that way unless a concrete need appears.
- The layout is the old .NET version's (`Name`, `DeviceId`, `MasterVolume`,
  `IsMasterMuted`, `IsFavorite`, `Settings[ProcessName, Volume, IsMuted]`), so its
  presets keep working. MixerPlus adds optional `InputDeviceId`, `InputVolume`, and
  `InputMuted`. Never rename or drop the legacy keys.
- Presets are global: one list, unique names (saving an existing name replaces it,
  keeping its label). `DeviceId` is kept only so the old version can still read the
  file; MixerPlus writes the current output there and never uses it. Applying sets the
  master level on the current default output, never switches device.
- A favorite (`IsFavorite`) is only a label. It must not affect applying, ordering,
  or anything automatic.
- Reads are bounded (1 MiB, 200 presets, 128 apps, depth 16); levels are clamped to
  0..1. An unreadable file is renamed to `presets.json.bad` before anything is
  saved. Writes go to `presets.json.tmp`, then rename. Treat the file as untrusted.

## Behavior

- Applications are matched by executable name without extension, case-insensitively
  (`Row::process`; System Sounds is `System Sounds`). Absent devices and
  applications are skipped silently; any other failure is reported once after
  everything else has been applied (`Audio::apply_preset`).
- Capturing stores the default output's level and mute, the default input's (with
  its ID), and every application row (`Audio::capture_preset`).
- Audio changes still require an explicit action: there is no automatic application
  (startup, output switch, polling). Never add one.

## Preset page

- `View::Presets`, opened by the slider glyph beside the gear. Rows use IDs
  `PRESET_ID + index * 4 + {0 apply, 1 favorite, 2 rename, 3 delete}`; `listed`
  maps an index to a name. Applying returns to the mixer so the result is visible.
  `show_view` posts its own refresh, so after it never post another: a second
  rebuild ends the running transition and the change looks abrupt.
- Delete takes two clicks (the first arms it, shown as a check). Saving and renaming
  use an inline EDIT box with a frame drawn by `appearance::set_edit_frame`: Enter
  commits (intercepted in the message loop before `IsDialogMessageW`), Escape
  cancels. A name that is empty or already taken keeps the box open.
