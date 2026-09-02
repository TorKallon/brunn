# Design QA — Still Water brand integration

Date: 2026-09-02
Result: **Passed**

## Visual target

The implementation was compared directly with the owner-approved A+ Still
Water composition and the selected Images 2.0 family: `r3-3` for the app icon,
`launch-corrected-1` for launch, and `hero-4` for the wide field. The mark keeps
one crescent-open rim, one off-centre point, and exactly three hairline ripples.

## Web

- The authenticated shell, authentication screens, and settings use the
  approved raster well at product scale; the wordmark remains live lowercase
  serif type.
- The favicon is the dedicated vector glyph, while the Apple touch icon uses
  the approved raster master. No image-model output contains type.
- The social card uses the approved wide field with a locally typeset lowercase
  wordmark and tagline at 1200 × 630.
- Light and night themes retain readable contrast and blue-only brand accents.
  The sidebar contains one still waterline; nothing animates.
- Desktop login, desktop workspace, and 390 px mobile navigation were inspected
  in a real Chromium render with no console or page errors. Mobile spacing was
  corrected so the close control cannot cover the mark.

## iOS

- `AppIcon.png` is the approved opaque, full-bleed 1024 × 1024 RGB master with
  no baked mask or transparency; the tinted appearance comes from the separate
  vector-derived monochrome asset.
- The static launch storyboard and in-app startup view both use
  `LaunchWaterline` as a full-bleed crop over `#030B18`, avoiding a visible
  square tile while keeping the composition inside the central vertical field.
- The miniature in-app mark is drawn in SwiftUI with the crescent, point, and
  three ripples rather than rasterized type.
- The icon and startup treatment were installed and inspected on an iPhone
  simulator after asset-catalog compilation.

## Engineering gates

- The deterministic generator reproduced every iOS and web derivative.
- Web production build passed; Vitest passed 25 files and 136 tests.
- Swift package tests passed 26 tests; the full simulator app build succeeded.
- Asset hashes, dimensions, opacity, SVG parsing, metadata, and retired-brand
  residue checks passed.
