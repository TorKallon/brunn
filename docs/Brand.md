# Brunn brand — Still Water

Status: adopted 2026-09-02; succeeds Night Signal. This is the
authoritative design system for every Brunn surface (web SPA, iOS app,
launch/app icons, future marketing).

## 1. Identity

### Name, pronunciation, and casing

- **brunn** comes from Old Norse *brunnr*, “well” (Mímisbrunnr, the well of
  memory beneath Yggdrasil). Brunn is a place and a thing, never an AI.
- Pronounce it as one syllable, **brun** (/brʊn/).
- The wordmark and product-surface brand signature are always lowercase
  `brunn`. Use `Brunn` only where English grammar demands a capital. Never use
  all-uppercase or camel-case forms.
- The descriptor is **agent-first workspace and memory**.
- The checkpoint and interchange format is **Brunn State**; its CLI, package,
  and filename form is `brunn-state`.

### Mark

The mark is **Still Water**: the mouth of a well seen directly from above at
night. A dark circular opening sits in a layered midnight field. Moonlight
catches only the upper-left lip as a short ice-to-cobalt crescent; the rim
never closes. In the water, one small point of light sits above-left of centre
inside exactly three still hairline ripples, each fainter than the last. The
point is what an agent dropped in, still shining when it comes back. Nothing
leaves the well; stillness is the promise.

The generated canonical masters are `assets/brand/brunn-well-2048.png` and
`assets/brand/brunn-well-1024.png` (opaque, full-bleed RGB, no baked corner
mask; platforms apply their own shape). `assets/brand/brunn-well.svg` is the
A+ composition reference, not the shipping raster artwork. At 180 and 120 px,
the mark reads as a well with a star in it. At 60 px, the ripples may vanish,
but the opening and point must remain. It must never read as a lens, eye,
speaker, portal, target, black hole, or disc.

Identity rules, from the approved concept board:

- The palette is **emphatically blue** — sapphire and indigo depth is welcome,
  restrained cool violet is welcome, pink/hot magenta/fuchsia are not.
- Exactly **one accent hue family** (signal blue). Everything else is quiet,
  cool-tinted neutral. Functional status colors (green/amber/red) exist for
  meaning, never for decoration.
- Dark surfaces are **chrome, not content**: navigation, hero, launch — the
  archive/void. Content lives in a light, airy workroom. Depth gradients and
  the full waterline composition belong only on night chrome.

### Wordmark

Set the single lowercase word **brunn** in
`Georgia, "Iowan Old Style", "Times New Roman", serif`, weight 500, tracking
`-0.03em`, using the appropriate `on-night` or `ink` token. Never apply a
gradient fill, outline, shadow, or rotation.

There is no ornamental wordmark variant. The light lives in the well, not over
the letters. The image model never renders the wordmark.

### Lockups, clearspace, and minimums

- **Horizontal, default:** mark at left; wordmark baseline-centered on the
  mark's vertical middle; gap 40% of mark height; wordmark cap height about
  55% of mark height.
- **Stacked, square or launch:** mark above the centered wordmark; gap 30% of
  mark height.
- **Clearspace:** at least the wordmark's *b*-ascender height on every side.
- **Minimums:** mark alone 16 px; lockup mark height 24 px; wordmark alone
  20 px cap height. Below those floors, show the mark alone.
- Do not put the depth treatment behind a lockup on the light workroom, and do
  not recolor the mark outside the night and signal ramps.

### Favicon and app-icon appearances

- At 48 px and below, use `assets/brand/brunn-well-glyph.svg`: a dedicated
  32-grid glyph with a centred point and complete ring. At 16 px it resolves to
  a 1 px stroke around a 2 px point. It deliberately omits the crescent,
  ripples, and bloom; a partial ring at that size looks like a spinner.
- `assets/brand/brunn-well-mono.svg` is the single-colour transparent mask for
  the iOS tinted appearance and hosts that supply their own tint. It contains
  only the ring and point.
- The standard iOS icon uses the generated night artwork. Ship the standard
  and tinted appearances; do not add a separate dark appearance because the
  mark is already night.

## 2. Color system

All pairings below were validated programmatically (WCAG 2.1 ratios; the data
palette additionally passed a six-check CVD validator: lightness band, chroma
floor, deutan/tritan separation ≥ 8 ΔE, normal-vision ΔE ≥ 15, ≥ 3:1 on
surface).

### Night ramp — dark chrome

| Token | Hex | Use |
| --- | --- | --- |
| `night-950` | `#030B18` | Launch background (dark), icon field edge |
| `night-900` | `#06152C` | Sidebar, hero base, launch background, `theme-color` |
| `night-800` | `#0B2144` | Nav hover, hero gradient mid |
| `night-700` | `#132D58` | Nav active, hero gradient end, chips on night |
| `night-edge` | `#102A50` | Borders of night surfaces against light surfaces |
| `night-line` | `#21385C` | Hairlines inside night surfaces |
| `night-line-bright` | `#294D89` | Emphasized borders on night (brand mark frame) |

Text on night: `on-night #F5F8FF` (primary), `on-night-muted #BDC9E2`
(secondary), `on-night-faint #9FB0D2` (metadata). All ≥ 8:1 on `night-900`.

### Signal ramp — the accent

| Token | Hex | Use |
| --- | --- | --- |
| `signal-700` | `#2444B7` | Hover/pressed of solid signal, link hover |
| `signal-600` | `#3158D9` | **Brand.** Solid buttons, links, active tabs, timeline dots |
| `signal-500` | `#4A6FE8` | Gradient bridge, radial glows on night |
| `signal-400` | `#607DFF` | Focus ring (only) |
| `signal-300` | `#8FA9FF` | Luminous accent on night: active-nav inset, kickers, hero primary button fill |
| `signal-200` | `#A9BCFF` | Hover of signal-300 fills; links on night |
| `signal-100` | `#C7D9FF` | Ice — crescent and waterline tint, decorative borders on light accent |
| `signal-soft` | `#E9EDFF` | Soft fills on light (notices, active chips) |
| `signal-wash` | `#F4F6FF` | Row hover, faintest wash |
| `signal-ink` | `#263F9E` | Text on `signal-soft` |
| `signal-line` | `#A9BCE8` | Borders of signal-soft fills |

Solid `signal-600` takes **white** text (5.95:1). Luminous `signal-300` fills
on night chrome take **night ink `#0A1638`** text (≥ 6:1) — never white.

### Fog ramp — light workroom neutrals

Cool, blue-tinted neutrals. The green-tinted grays of the forest era are
retired everywhere.

| Token | Hex | Use |
| --- | --- | --- |
| `bg` | `#F5F6FA` | App background |
| `surface` | `#FFFFFF` | Cards, sections, tables |
| `surface-subtle` | `#F0F2F8` | Hovers, disabled fills, count chips |
| `surface-strong` | `#E4E8F2` | Icon wells, emphasized chips |
| `code-bg` | `#F5F7FB` | Code, pre, source panes, inset forms |
| `line` | `#D9DDE8` | Hairlines |
| `line-strong` | `#BCC4D6` | Input borders, emphasized rules |
| `ink` | `#1B2130` | Headings, primary text (16.1:1) |
| `ink-2` | `#3E4557` | Prose body (9.6:1) |
| `ink-3` | `#4A5468` | Field labels, badge text (7.6:1) |
| `muted` | `#5D6780` | Secondary text (5.7:1) |
| `faint` | `#6B7690` | Smallest metadata (4.6:1 — still AA) |
| `code-ink` | `#232B3B` | Text on `code-bg` |

### Dark workroom — the default appearance

The web app ships **dark by default** (owner preference: the product is read at
night). The dark workroom is deliberately **low-glare**: no pure white text, no
pure black grounds; body text sits near 12–13:1, never 15+. Night chrome
(sidebar, hero) keeps its `night-900` identity in both appearances, so dark
mode reads as the workroom dimming to meet the chrome. Light remains available
behind `data-theme="light"` on the root element. Web and iOS expose an explicit
Dark/Light choice in Settings and save it locally; the OS
`prefers-color-scheme` is deliberately not consulted.

| Token | Dark value | Light value | Note |
| --- | --- | --- | --- |
| `bg` | `#0A1322` | `#F5F6FA` | |
| `surface` | `#101B30` | `#FFFFFF` | |
| `surface-subtle` | `#172441` | `#F0F2F8` | |
| `surface-strong` | `#1F2E51` | `#E4E8F2` | |
| `code-bg` / `code-ink` | `#0D1626` / `#C6D1E8` | `#F5F7FB` / `#232B3B` | |
| `line` / `line-strong` | `#223255` / `#31446E` | `#D9DDE8` / `#BCC4D6` | |
| `ink` | `#D7DFEE` | `#1B2130` | 12.8:1 — comfort ceiling |
| `ink-2` / `ink-3` | `#C6D0E4` / `#AFBAD4` | `#3E4557` / `#4A5468` | |
| `muted` / `faint` | `#93A0BE` / `#8894B2` | `#5D6780` / `#6B7690` | all ≥ 4.5:1 |
| `link` (text accent) | `#8FA9FF` | `#3158D9` | split from fills |
| `accent-fill` (+hover) | `#3B62E0` (`#4166E6`) | `#3158D9` (`#2444B7`) | white text on both |
| `danger-fill` | `#B4444F` | `#A23B3B` | white text on both; `--red` stays a text color |
| `focus` | `#7C93FF` | `#607DFF` | |
| `signal-soft/ink/line/wash` | `#1C2B52` / `#B7C6FF` / `#3A5292` / `#141F38` | `#E9EDFF` / `#263F9E` / `#A9BCE8` / `#F4F6FF` | |
| success | `#66C695` on `#12291E`, line `#23503B` | see status table | |
| warning | `#D9A251` on `#2C2210`, line `#57431D` | | |
| danger | `#E08894` on `#2E181C`, line `#5C2E35` | | |
| `data-read` / `data-write` | `#5C7BEA` / `#2AA48F` | `#3158D9` / `#0F97A8` | dark pair re-validated (deutan ΔE 18.3) |
| `chart-grid` / `data-neutral` | `#1B2946` / `#7C88A6` | `#ECEFF5` / `#8A96AC` | |
| `field-bg` (inputs) | `#0D1727` | `#FFFFFF` | |
| `topbar-bg` | `rgb(13 22 40 / 93%)` | `rgb(255 255 255 / 96%)` | |

Accent discipline in the dark workroom: **text accents use `link`
(`signal-300`), solid fills use `accent-fill` with white text** — never white
text on `signal-300`, never `signal-600` text on dark surfaces.

### Status colors — meaning only

| Role | Text/solid | Deep (on soft) | Soft fill | Border |
| --- | --- | --- | --- | --- |
| Success | `#177A4F` | `#14603E` | `#E3F3EA` | `#9CC9B4` |
| Warning | `#8B5B09` | `#8B5B09` | `#FAF0D9` | `#DBBA7B` |
| Danger | `#A23B3B` | `#A23B3B` | `#FAE8EA` | `#DBA1A8` |
| Info | signal family (`signal-600` on `signal-soft`, border `signal-line`) | | | |

Success is green again — it is a status, not the brand. The brand is never
used to mean "success", and status colors are never used decoratively.

### Data palette — charts

| Series | Hex | Note |
| --- | --- | --- |
| Series 1 (reads/primary) | `#3158D9` | signal-600 |
| Series 2 (writes/secondary) | `#0F97A8` | pulse teal — passes all six categorical checks against series 1 |
| Chart grid | `#ECEFF5` | recessive |
| Neutral marker | `#8A96AC` | |

Fixed assignment order, never cycled; a third series requires revisiting the
palette with the validator, not improvising. Legends and value labels wear
text tokens, never series color. iOS text-safe pulse: light `#0F7583`,
dark `#5FB9D0`.

## 3. Typography

| Role | Face | Size / weight / tracking | Use |
| --- | --- | --- | --- |
| Wordmark | Georgia stack | free · 500 · −0.03em | the word `brunn` only |
| Display | Georgia stack | 2.2–3.4rem · 500–600 · −0.03em · line-height 1.1 | hero, auth, marketing |
| Title | Georgia stack | 1.7rem · 600 · −0.02em · line-height 1.2 | page and section headings |
| Subtitle | Georgia stack | 1.35rem · 600 · −0.02em · line-height 1.3 | brand-surface card headings |
| Body | Inter/system sans | 1rem · 400 · line-height 1.6 | prose |
| UI | Inter/system sans | 0.8–0.9rem · 500 | controls, tables, navigation |
| Label | Inter/system sans | 0.66–0.7rem · 600 · +0.14–0.22em · uppercase | eyebrows and badges |
| Caption | Inter/system sans | 0.78rem · 400 | metadata using `faint`, still ≥ 4.5:1 |
| Mono | ui-monospace stack | 0.86em · 400 | identifiers and paths; tabular figures for data |

iOS uses the system font (SF Pro); brand moments may use `.fontDesign(.serif)`.
The serif voice is editorial and sparing — headlines and the wordmark, never
body text or controls. Build hierarchy with size, ink tokens, and space rather
than weight 700. Every numeric table uses tabular figures.

## 4. Shape, space, elevation

- Radius: `6px` controls and small chips, `10px` cards and charts, `16px` hero
  and marketing surfaces. iOS: 8 pt cards, brand mark corner radius = 28 % of
  its size (matches the app-icon squircle feel).
- Shadows are night-tinted, never gray-green: menus
  `0 8px 24px rgb(11 22 52 / 14%)`, hero `0 18px 50px rgb(6 21 44 / 16%)`,
  login panel `0 12px 36px rgb(11 22 52 / 14%)`.
- Density stays as-is: compact tables, 0.7–0.9 rem UI text, generous page
  gutters (24–28 px desktop, 12–18 px phone).

## 5. Motifs

1. **Point source** — small filled dots mark "now/current": timeline nodes,
   status dots. Always `signal-600` on light, white/`signal-300` on night.
2. **Signal edge** — a 3 px left accent bar means "surfaced for you"
   (briefing summary, today metrics). `signal-600` default, `pulse` for
   write-flavored data, neutral `#8A96AC` otherwise.
3. **Depth** — on night chrome, a vertical darkening toward the bottom
   (`#0A1322 → #030B18`) plus one soft sapphire glow at the upper left. Never
   use the retired diagonal luminous treatment. Never animate it.
4. **Night chrome** — exactly one dark orientation surface per screen
   (sidebar or hero), giving the depth treatment one quiet place to live.
5. **Waterline** — on hero and launch, one ice-white horizontal hairline with
   the point above, its reflection beneath at about 55% brightness, and at most
   three static ripple hairlines that widen about 1.7× and fade outward. In the
   workroom, use only the bare 1 px hairline, at most once per screen, beneath
   the sidebar brand block. No reflection or ripples there. Still, always.

## 6. Component recipes (web)

- **Primary button**: `signal-600` fill, white text; hover `signal-700`.
  On night chrome: `signal-300` fill, `#0A1638` text; hover `signal-200`.
- **Secondary button**: `surface` fill, `line-strong` border, `ink` text;
  hover `surface-subtle`. On night: `rgb(255 255 255 / 8%)` fill, `on-night`
  text.
- **Danger button**: solid `#A23B3B`, white text.
- **Focus**: 2 px `signal-400` ring, 2 px offset, everywhere.
- **Badges/chips**: uppercase 0.66 rem, `surface-subtle` + `ink-3` neutral;
  tone variants use the status table above.
- **Notices**: soft fill + matching border + deep text (e.g. readonly notice =
  `signal-soft`/`signal-line`/`signal-ink`).
- **Active nav**: `night-700` fill, white text, 2 px `signal-300` inset bar.
- **Inputs**: white fill, `line-strong` border; focus border `signal-600` +
  soft `rgb(49 88 217 / 18%)` outer ring; invalid = danger border + soft fill.

## 7. Platform mappings

### Web (`apps/web/src/styles.css`)

Tokens live as CSS custom properties on `:root` (`--night-900`, `--brand`,
`--signal-300`, `--green-soft`, …). `--brand`/`--brand-hover`/`--brand-soft`
remain the working aliases of `signal-600/700/soft`. Chart series use
`--data-read`/`--data-write`. No green-era hexes may survive; new rules must
use tokens, not literals.

The dark workroom is applied via `:root:not([data-theme="light"])` overriding
the light tokens, so **dark is the default** and `data-theme="light"` restores
the light workroom without duplicating component rules. `color-scheme` follows
the active appearance so native controls and scrollbars match. Text accents
must reference `--link`, solid fills `--accent-fill`; neither may hard-code
`--brand` for text on dark surfaces.

### iOS (`apps/ios/Brunn/Shared/BrunnTheme.swift`)

`BrunnTheme` exposes light/dark-adaptive colors via dynamic providers:

| Token | Light | Dark |
| --- | --- | --- |
| `signal` (accent) | `#3158D9` | `#8FA9FF` |
| `ink` | `#1B2130` | `#E7EDF9` |
| `pulse` (secondary accent) | `#0F7583` | `#5FB9D0` |
| `amber` | `#8B5B09` | `#D9A251` |
| `red` | `#AC3B47` | `#E08894` |
| `night` (chrome) | `#06152C` | `#030B18` |

`AccentColor` mirrors `signal`. The static launch storyboard and in-app startup
view both fill the screen with generated `LaunchWaterline` artwork over
`LaunchBackground` (`#030B18`), using an aspect-fill crop so the square source
never becomes a visible tile. Neither layer animates or rasterizes type. The
in-app `BrandMark` is a SwiftUI-drawn miniature of the well geometry (depth,
opening, upper-left crescent, point, and three quiet ripples), not a lettermark.
`BrunnTheme.well` is `#02060F` and is used only for the mark interior and hero
depth floor. `forest` is retired.

## 8. Accessibility contract

- Body and metadata text ≥ 4.5:1 on its background; headings ≥ 7:1;
  decorative/duplicated glyphs ≥ 3:1.
- Status is never color-alone: badges carry text, icons accompany danger.
- Focus is always visible (`signal-400` ring on light and night).
- Charts: two named series max without re-validation, direct labels or legend
  always present, series identity survives deutan/tritan simulation.
- Honor `prefers-reduced-motion`; Still Water never animates.

## 9. Voice

Brand copy uses **drops in** for capture, **draws up** for retrieval,
**the well** for the corpus, **holds** for durability, and **still** for the
promise that nothing moves on its own. Avoid database vocabulary such as
“stored” and “retrieved” in brand copy.

Product examples: “Nothing in the well yet.” · “Drawn up from 3 sources.” ·
“Held.” · “The well is out of reach.”

Approved one-line directions include:

- “The well your agents draw from.”
- “Memory that holds.”
- “Everything you've kept, still there when you come back.”

## 10. Asset inventory and regeneration

- Generated masters: `assets/brand/brunn-well-2048.png`,
  `assets/brand/brunn-well-1024.png`, `assets/brand/brunn-waterline-1024.png`,
  and `assets/brand/brunn-hero-wide.png`.
- Vector masters: `assets/brand/brunn-well.svg` (A+ composition reference),
  `assets/brand/brunn-well-glyph.svg` (≤ 48 px), and
  `assets/brand/brunn-well-mono.svg` (host-tinted mask).
- Derived iOS assets: `AppIcon.png`, `AppIcon-tinted.png`,
  `LaunchWaterline.imageset`, and `LaunchBackground.colorset`.
- Derived web assets: `favicon.svg`, `favicon-32.png`, `favicon-16.png`,
  `apple-touch-icon.png`, `og.png`, and the in-app raster mark.

Regenerate derivatives with the single script
`apps/ios/Tools/generate_app_icon.swift`; it accepts the canonical raster and
vector masters and emits the iOS and web sizes. WebP output requires `cwebp`
from the WebP tools (`brew install webp` on macOS). The image model is never
used for the favicon glyph, mono mask, wordmark, or any other type.
