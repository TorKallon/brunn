# brunn brand assets — Still Water

Still Water is the production identity adopted on 2026-09-02. Superseded
artwork is removed from the tree; Git history remains its recovery path.

## Raster masters

These are the owner-approved, opaque RGB masters. Keep their bytes unchanged;
platform-specific files are derived from them without adding type or a corner
mask.

| Asset | Dimensions | SHA-256 |
| --- | --- | --- |
| `brunn-well-2048.png` | 2048 × 2048 | `16ceb7c14e4f2293b4bd17dfe4016be7be55f11fb25cb7a42c9edb701e9aa535` |
| `brunn-well-1024.png` | 1024 × 1024 | `d3b1673bc7cec6ab56f5181d788764e9a3ef0b4f282e466200812ccca9d4f0fc` |
| `brunn-waterline-1024.png` | 1024 × 1024 | `376ba024b6a89862d2d45ae41d447df271938e3e57b9037a931167842c232a49` |
| `brunn-hero-wide.png` | 3840 × 2160 | `77c283768ef4651fbdd95d8299d583f1f791750719621a4d340ae281bbe72384` |

`brunn-well-1024.png` is the canonical app-icon master. It is full bleed and
has no baked platform mask. `brunn-waterline-1024.png` is launch artwork on
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
