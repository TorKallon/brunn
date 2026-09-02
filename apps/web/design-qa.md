# Still Water web design QA

## Evidence

- Source visual truth:
  - `assets/brand/brunn-well-1024.png` — approved 1024 × 1024 mark.
  - `assets/brand/brunn-hero-wide.png` — approved type-free hero field.
  - `docs/Brand.md` — depth, waterline, glyph, type, and copy contract.
- Browser-rendered implementation:
  - transient login, workspace, mobile-navigation, and source-comparison
    captures were kept outside the repository during verification.
- Viewports: desktop 1440 × 900 and 1440 × 1000 CSS px; mobile 390 × 844 CSS px.
- Density normalization: all browser captures used device scale factor 1. The 1024 px mark was Lanczos-downsampled to its 32 px rendered size for the focused comparison; both 32 px samples were then nearest-neighbor enlarged equally for inspection. OG source and implementation are both 1200 × 630 and were compared at equal scale.
- State: dark-default login; authenticated dark workroom with deterministic mocked content; mobile sidebar open and closed.

## Findings

No actionable P0, P1, or P2 mismatches remain.

- Typography: the visible wordmark is lowercase Georgia with the specified weight and tracking; product copy remains system sans. The OG wordmark is locally typeset in Georgia and no model-made type is present.
- Spacing and layout: desktop layout and density are unchanged. The mark fits its existing 32 px slot without crop or distortion. The single sidebar waterline is aligned directly below the brand block. Mobile brand spacing now clears the navigation toggle.
- Colors and tokens: the new `--well` value is exact. The sidebar uses the specified vertical depth gradient from `#0A1322` to `#030B18` with one restrained upper-left sapphire glow. No forbidden brand color appears in the added UI or assets.
- Image quality: the in-app WebP preserves the approved mark's crescent, point, and depth; the three ripples intentionally disappear at this small size. The 16/32 favicons use the dedicated vector glyph rather than a downscaled model image. The touch icon and OG are opaque RGB at their required dimensions.
- Copy: `brunn` remains lowercase. The OG uses “The well your agents draw from.” and metadata matches the same voice.

## Interaction and browser checks

- Filled and cleared the login fields and verified Show password / Hide password changes the field state.
- Opened and closed the mobile navigation and verified its expanded state.
- Requested all six public brand assets successfully.
- Verified the SVG favicon precedes both PNG fallbacks in the rendered document.
- Checked browser console errors, page errors, and failed requests: none.

## Comparison history

1. Initial mobile capture found a P2 overlap between the existing fixed close toggle and the sidebar mark.
2. Added mobile-only left spacing to the sidebar brand block.
3. Recaptured at 390 × 844: toggle box `x=10…44`; mark box `x=56…88`; no overlap remains, and the waterline and wordmark stay inside the 232 px sidebar.

## Residual test gap

- Headless page screenshots do not include browser chrome, so the favicon itself was not visually photographed inside a native tab. Chromium did load the vector-first link order and all favicon resources successfully; the 16 px and 32 px raster outputs were also inspected directly on light and night grounds during asset review.

## Implementation checklist

- [x] Approved mark in auth and sidebar.
- [x] Vector-first favicon and PNG fallbacks.
- [x] Apple touch icon and locally typeset OG.
- [x] Depth gradient, well token, and one sidebar waterline.
- [x] Desktop/mobile responsive and interaction check.
- [x] No console or page errors.

final result: passed
