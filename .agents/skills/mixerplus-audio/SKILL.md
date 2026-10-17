---
name: mixerplus-audio
description: Rules for mixerplus Core Audio work - COM lifetime, endpoint and session enumeration, volume, mute, and default-device switching in src/audio.rs.
---

# mixerplus audio

- Keep COM interfaces on their owning initialized UI thread and release them
  before `CoUninitialize`. Free Windows-owned strings and clear PROPVARIANT values.
- Treat endpoint removal, session expiration, absent microphones, and process
  metadata access denial as normal operating conditions, not errors to report.
- Never change system audio during startup or background polling. Volume, mute,
  and default-device changes must follow an explicit user interaction.
- Like the Windows volume mixer, each application has one row across all active
  outputs, grouped by executable path (process ID when the path is inaccessible).
  Sessions on the default output come first and supply the displayed level.
  Names come from the session display name (resolving `@` resources), then the
  executable's file description, then its file name; they are cached per row.
- Endpoint lists, defaults, names, and activated endpoint interfaces are cached in
  `Audio::endpoints`, since each default or list query is a millisecond round trip
  to audiosrv. An `IMMNotificationClient` only bumps a change counter (it runs on
  a Windows thread); the popup's opening and `set_default` call `Audio::forget`.
  Sessions are still enumerated on every refresh.
- Peak levels come from `IAudioMeterInformation`, one per endpoint or session;
  `Row::peak` returns the maximum. Meters are plain reads and never change audio.
- Presets capture and apply through `Audio::capture_preset` and `apply_preset`;
  see `mixerplus-presets` for matching and failure rules.
- Keep undocumented PolicyConfig calls isolated in `audio.rs`; the ten vtable
  slots before `SetDefaultEndpoint` must keep their order. Surface operation
  failures and refresh devices; never silently report a successful switch.
- Spatial sound uses the documented WinRT `SpatialAudioDeviceConfiguration`, keyed by
  the endpoint's device interface path and cached for the last output. The empty
  GUID (`SPATIAL_OFF`) turns it off. Dolby and DTS report support even when their
  apps are absent, so only Windows Sonic and the current format are offered.
  Changes follow a click only and surface a refused status as an error.
- Use `--check-audio` for read-only enumeration. Device-switch behavior needs
  the actual devices and must be reported as unverified when they are absent.
