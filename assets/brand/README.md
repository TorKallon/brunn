# brunn brand assets — Still Water

Still Water is the production identity adopted on 2026-09-02. The icon's
sapphire surround and sharper geometry were approved on 2026-09-12.
Superseded artwork is removed from the tree; Git history remains its recovery path.

## Raster masters

These are the opaque RGB sources and generated master. Platform-specific
files are derived without adding type or a corner mask.

| Asset | Dimensions | Role |
| --- | --- | --- |
| `brunn-well-source.png` | 1254 × 1254 | Native generated source; keep its bytes unchanged |
| `brunn-well-1024.png` | 1024 × 1024 | Derived app-icon master |
| `brunn-waterline-1024.png` | 1024 × 1024 | Launch source |
| `brunn-hero-wide.png` | 3840 × 2160 | Hero source |

`brunn-well-1024.png` is generated from `brunn-well-source.png` by the same
script that emits all platform sizes. It is full bleed, with blue extending
into every corner, and has no baked platform mask or external shadow. The
approved sapphire shoulder and edge lighting remain within the artwork.
The production-source prompt is in `brunn-well-source-prompt.md`.
Web references use `?v=20260912` for the touch icon and raster mark so the
refinement bypasses the previous assets' immutable browser and CDN caches.
Change this version whenever those assets change.
`brunn-waterline-1024.png` is launch artwork on
`#030B18`. `brunn-hero-wide.png` is artwork only; web copy remains locally
typeset by `apps/ios/Tools/generate_app_icon.swift`. Full derivative
regeneration requires the WebP tools (`brew install webp` on macOS).

## Vector masters

- `brunn-well.svg` is the owner-approved A+ composition reference: an open
  upper-left crescent, one off-centre point, and three hairline ripples.
- `brunn-well-glyph.svg` is the dedicated full-colour glyph for sizes at or
  below 48 px.
- `brunn-well-mono.svg` is the single-colour transparent mask for tinted and
  pinned-tab uses; the host supplies its colour.

The vector glyph, mono mask, and code-set lowercase `brunn` wordmark are never
rasterized by the image model. Exploration files stay outside the repository.
