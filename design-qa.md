# Design QA — Night Signal brand integration

Date: 2026-08-02
Result: **Passed**

## Visual target

The implementation was compared directly with the user-approved concept board
at `assets/brand/reference/straylight-night-signal-selected-board.png`.

## Web

- The real raster mark replaces the placeholder `S` on both the authenticated
  shell and authentication screens.
- The visible identity uses a midnight sidebar, blue interaction accents, and
  a readable serif system stack for `Straylight`; green is no longer used as a
  core web brand color.
- The production assets stay blue-dominant while using layered sapphire/cobalt
  shading and internal beam gradations; no visible pink or hot magenta remains.
- The 64 px favicon remains recognizable when reduced to 32 px and 16 px.
- The login implementation was captured at
  `/Users/aether/.codex/visualizations/2026/08/02/019fc465-35c6-7671-ae30-6cc291b72b59/straylight-web-brand-login.png`.
- Source and implementation were inspected together: the off-center source,
  single lower-right beam, midnight field, and serif wordmark all remain
  faithful at product scale, with no clipping or layout regression.

## iOS

- `AppIcon.png` is an opaque, full-bleed, 1024 × 1024 RGB PNG with no baked
  corner mask or transparency.
- The source and beam remain inside the system-mask safe region and read at
  small icon sizes.
- `LaunchSignal` provides transparent 1×, 2×, and 3× artwork with generous
  padding; `LaunchBackground` provides explicit default and dark navy colors.
- The asset catalog compiles without icon or image-set warnings in an unsigned
  generic-device build.

## Engineering gates

- Web production build passed and emitted the logo, favicon, and Apple touch
  icon into `dist`.
- Web test suite passed: 13 files, 66 tests.
- Unsigned generic iOS device build passed after the app-icon replacement.
