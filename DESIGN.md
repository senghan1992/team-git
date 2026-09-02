---
name: Git Companion
description: Cobalt-white porcelain (청화백자) — a quiet, precious record-keeping instrument for the daily git loop.
colors:
  porcelain-canvas: "#f6f5f1"
  porcelain-deep: "#efede5"
  clay: "#ece8dd"
  clay-strong: "#e1dccb"
  plaque: "#ffffff"
  plaque-wash: "#faf9f5"
  clay-wash: "#f1eee6"
  ink: "#25272c"
  ink-muted: "#6c6a63"
  ink-faint: "#8f8a7d"
  cobalt: "#2c4b8f"
  cobalt-hover: "#24407a"
  cobalt-deep: "#1c3260"
  cobalt-wash: "#eef1f8"
  celadon: "#276b4e"
  iron: "#8a5a10"
  copper: "#ad392c"
  copper-deep: "#8f2d22"
  hairline: "#e6e1d4"
  border-strong: "#d2cbb9"
  border-hover: "#c2bba9"
  toast: "#2a2d34"
  toast-celadon: "#83c9a5"
  toast-copper: "#e79a8a"
  toast-cobalt: "#a3b9e4"
  cta-top: "#32518f"
  cta-bottom: "#21396b"
  cta-edge: "rgba(18, 34, 66, 0.5)"
typography:
  display:
    fontFamily: "Pretendard Variable, Pretendard, Apple SD Gothic Neo, Noto Sans KR, sans-serif"
    fontSize: "28px"
    fontWeight: 700
    letterSpacing: "-0.02em"
  headline:
    fontFamily: "Pretendard Variable, Pretendard, Apple SD Gothic Neo, Noto Sans KR, sans-serif"
    fontSize: "22px"
    fontWeight: 600
    letterSpacing: "-0.01em"
  title:
    fontFamily: "Pretendard Variable, Pretendard, Apple SD Gothic Neo, Noto Sans KR, sans-serif"
    fontSize: "16px"
    fontWeight: 700
    letterSpacing: "-0.01em"
  body:
    fontFamily: "Pretendard Variable, Pretendard, Apple SD Gothic Neo, Noto Sans KR, sans-serif"
    fontSize: "14px"
    fontWeight: 400
  label:
    fontFamily: "Pretendard Variable, Pretendard, Apple SD Gothic Neo, Noto Sans KR, sans-serif"
    fontSize: "11px"
    fontWeight: 600
  mono:
    fontFamily: "ui-monospace, SF Mono, Cascadia Mono, JetBrains Mono, Menlo, Consolas, monospace"
rounded:
  xs: "6px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  plate: "10px"
  tab: "7px"
  full: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  base: "16px"
  lg: "24px"
  xl: "32px"
  xxl: "40px"
  xxxl: "48px"
components:
  button-primary:
    backgroundColor: "{colors.cobalt}"
    textColor: "#ffffff"
    rounded: "{rounded.sm}"
    padding: "0 24px"
    height: "40px"
  button-primary-hover:
    backgroundColor: "{colors.cobalt-hover}"
    textColor: "#ffffff"
    rounded: "{rounded.sm}"
    padding: "0 24px"
    height: "40px"
  button-primary-disabled:
    backgroundColor: "{colors.clay-strong}"
    textColor: "{colors.ink-faint}"
    rounded: "{rounded.sm}"
    padding: "0 24px"
    height: "40px"
  button-secondary:
    backgroundColor: "{colors.plaque}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "0 20px"
    height: "40px"
  button-cta:
    backgroundColor: "#faf8f2"
    textColor: "{colors.cobalt-deep}"
    rounded: "{rounded.sm}"
    padding: "0 24px"
    height: "40px"
  input:
    backgroundColor: "{colors.plaque}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "0 16px"
    height: "40px"
  card:
    backgroundColor: "{colors.plaque}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "24px"
  nav-item-active:
    backgroundColor: "{colors.plaque}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "8px 12px"
  badge:
    backgroundColor: "{colors.clay}"
    textColor: "{colors.ink-muted}"
    rounded: "{rounded.full}"
    padding: "0 9px"
    height: "22px"
  status-chip:
    backgroundColor: "{colors.plaque}"
    textColor: "{colors.ink-muted}"
    rounded: "{rounded.sm}"
    padding: "4px 12px"
  tab:
    backgroundColor: "transparent"
    textColor: "{colors.ink-muted}"
    rounded: "7px"
    padding: "0 16px"
    height: "30px"
  tab-active:
    backgroundColor: "{colors.plaque}"
    textColor: "{colors.ink}"
    rounded: "7px"
    padding: "0 16px"
    height: "30px"
---

# Design System: Git Companion

## Overview

**Creative North Star: "청화백자 — the cobalt-white porcelain workbench"**

Git Companion is drawn as a precious record-keeping object rather than a gray SaaS dashboard: a moon-jar porcelain body with one cobalt underglaze accent, where the daily git loop — committing, pushing, merging, stashing — is the act of inscribing durable records on porcelain. Every surface is a ceramic material, never flat neutral: the workspace canvas is the warm-white porcelain body, cards are glazed plaques, the sidebar is unfired clay, and state is "engraved" into the surface with 1px hairlines and hatch/cross-hatch marks so color is never the sole signal (a deliberate donation from instrument-world challengers at direction time: state must survive without color).

The system is deliberately quiet: one saturated accent (cobalt), ceramic-status hues (celadon / iron / copper) held at legible-but-muted depth, warm-gray ink instead of pure black, and shadows soft enough to read as light falling on glaze. Density stays at developer-tool level — ledgers, tabular counters, mono hashes, tight rows — so the warmth never slides into decoration. The one authored motion moment is the *fired-glaze sheen* that sweeps a primary button while it is busy, and the toast that enters like a glaze setting (blur-to-sharp settle). The old pink-on-white travel-inventory look is the confirmed anti-reference.

**Key Characteristics:**
- One saturated accent — cobalt underglaze — used structurally (primary plaques, active ticks), never as decal.
- Materials over colors: porcelain canvas, glazed plaques, clay sidebar, engraved hairlines.
- State reads by pattern as well as color: cross-hatch = conflict, hatch = staged/active marks, solid = clean.
- Signed tabular counters (↑n ↓n) and mono measurement chips instead of decorative stats.
- Soft porcelain light: every shadow carries offset + blur; no hard or colored shadows.
- Korean-first type: Pretendard Variable, one grotesque for display and body, system mono for paths/hashes/counters.

## Colors

The palette is ceramic glazes on porcelain: cool-warm white ground, ink that leans blue-black, one cobalt accent, and three muted glaze hues for status.

### Primary
- **Cobalt Underglaze** (#2c4b8f): The single accent. Primary buttons, the active nav tick and icon, focus rings, the Home CTA plaque, link colors. Hover deepens to **Cobalt Fired** (#24407a); pressed to **Cobalt Raw** (#1c3260). A 3px ring at 14% cobalt marks input focus; **Cobalt Wash** (#eef1f8) fills selected rows and info banners.
- **Celadon** (#276b4e): success — clean state, adds, merge-complete, 🜁 `↑n` ahead counters. On **Celadon Wash** (#edf4ef).
- **Iron** (#8a5a10): warning — modified files, "기본과 다름", degraded states. On **Iron Wash** (#f8f1e1).
- **Copper** (#ad392c): danger — conflicts, deletions, destructive actions (hover **Copper Deep** #8f2d22). On **Copper Wash** (#f9eeec), and always double-encoded with the cross-hatch pattern.

### Neutral
- **Porcelain Body** (#f6f5f1): workspace canvas — deliberately not pure white; a whisper of warmth so white plaques can float on it.
- **Porcelain Deep** (#efede5): hover washes on the canvas (action cells, card hovers).
- **Clay** (#ece8dd): the sidebar and unglazed chips; **Clay Strong** (#e1dccb) for pressed/disabled.
- **Plaque** (#ffffff): glazed surfaces — cards, inputs, modals, active nav, active tabs, toasts are dark.
- **Ink** (#25272c): body text, headings — blue-black, never pure black.
- **Ink Muted** (#6c6a63): secondary text and placeholders (≥4.5:1 on plaque and porcelain).
- **Ink Faint** (#8f8a7d): disabled text and quiet strokes only.
- **Engraved Hairline** (#e6e1d4): 1px borders and rules; **Strong Hairline** (#d2cbb9) for inputs and hover borders.

### Named Rules
**The One Cobalt Rule.** Cobalt is scarce: one saturated block per screen (the Home CTA is the only full-cobalt plaque in the app). Everything else earns cobalt only as a mark — a tick, a ring, a tile, an icon.

**The Pattern-Carry Rule.** No state lives in color alone: conflict badges and conflict file items double-encode with the cross-hatch mark, the active nav item carries a carved cobalt tick, and staged/busy marks use hatched engraving. A user who cannot see hue still reads every state.

## Typography

**Display/Body Font:** Pretendard Variable (bundled woff2, 45–920 axis; fallbacks Pretendard → Apple SD Gothic Neo → Noto Sans KR → sans-serif)
**Mono Font:** ui-monospace stack (SF Mono / Cascadia Mono / JetBrains Mono / Menlo / Consolas)

**Character:** One Korean grotesque carries display and body alike — quiet, precise, slightly tightened at display sizes. Mono is reserved for things that are measured or addressed: paths, short SHAs, branch names, signed counters, join codes, and proof reports.

### Hierarchy
- **Display** (700, 28px, -0.02em): page titles — "저장소 목록", repository names, the login gate title.
- **Headline** (600, 22px, -0.01em): modal titles, empty-state titles, section leads in settings.
- **Title** (700, 16px, -0.01em): the sidebar wordmark, card headers at 16px.
- **Body** (400, 14px): table rows, badges' neighbors, descriptions; secondary copy at 13px.
- **Label** (600, 11px, -0.01em): action-bar labels, section labels like "등록된 저장소".
- **Mono** (400, 12–13px): the status chip "↑0 ↓2 12개 파일", hash chips (`gc-hash-chip`, 12px), paths.

### Named Rules
**The Measurement Rule.** Numbers that mean something are set in mono with `font-variant-numeric: tabular-nums` — counters, SHAs, ports, latencies. Numerals in a ledger never jitter.

## Layout

The shell is a fixed sidebar + scrollable main column (240px sidebar, 32px page padding, 24px vertical rhythm groups, 4px spacing base). Navigation is left: 저장소 / 팀 / 설정 over the clay field, with the registered-repo ledger and the account chip pinned below a hairline. Views are single-column stacks of plaques; the repository grid is 2-up on wide windows, 1-up below `md` (768px). Cards group at 24px padding with 12–16px gaps between; tables breathe at 12–16px cell padding with 1px hairline rules (no zebra striping). Content never centers or letterboxes — a desktop workbench uses its width. The Home view leads with the cobalt CTA plaque, then the repo grid; the repo view leads with title → underglaze tabs → branch/status row → status ledger → commit-action plaque. More space sits above a heading than below it.

## Elevation & Depth

Porcelain light: depth is the soft shadow of glazed plaques floating over the porcelain body, never hard or colored. Cards, active nav items, active tabs, and the popover modal carry offset + blur shadows; the modal's `--shadow-pop` is the deepest layer in the app. The cobalt CTA gets a glaze highlight (`inset 0 1px 0` white at 14–16% top light) plus a soft cobalt drop. Buttons press down 0.5px on `:active` with an inner shade. There are no zero-blur block shadows, no colored halos, no glow.

### Shadow Vocabulary
- **Plaque Rest** (`0 1px 2px rgba(37,39,44,.04), 0 3px 10px rgba(37,39,44,.05)`): cards, active nav, active tabs, status chips.
- **Kiln Float** (`0 4px 12px rgba(37,39,44,.08), 0 16px 40px rgba(37,39,44,.14)`): modals, the SSH browser.
- **Fired Toasts** (`0 1px 1px rgba(37,39,44,.05), 0 10px 28px rgba(37,39,44,.12)`): the dark porcelain toast.

## Shapes

The form language is the moon jar's curve: gentle but not pill-soft. Buttons, inputs, nav rows, and chips use the 8px `sm` radius; cards, banners, the empty-state tile, and popovers use 12px `md`; the CTA, modal, and gate plaque use 16px `lg`; badges and the loading ring are full circles. The one 90° note is the cobalt tick — a 3×15px rounded 2px bar carved left of the active nav item, and the cross-hatch marks on conflict chips. Borders are 1px engraved hairlines; a colored edge, where one exists (banners), is exactly 1px and never more. The CTA plaque carries a radial glaze sheen (`radial-gradient` top-left, white 14%) — light falling on glaze, not a gradient-text effect.

## Components

### Buttons
- **Shape:** 8px radius (`sm`), 40px height.
- **Primary:** cobalt plaque (#2c4b8f) with white 600-weight text, `inset 0 1px 0` glaze highlight, soft cobalt drop shadow; hover darkens to #24407a; active presses to #1c3260 with an inner shade. Disabled is unglazed clay (clay-strong bg, faint-ink text).
- **Busy:** the *fired-glaze sheen* — a white 26%-opacity band sweeps across the plaque once per 1.15s while `[aria-busy]`; disabled during work.
- **Secondary:** white plaque, 1px strong-hairline border, ink text; hover warms to plaque-wash with a darker hairline.
- **CTA button:** white-on-cobalt inverse — plaque #faf8f2 with cobalt-deep text, the Home plaque's only light element besides type.

### Chips
- **Badge:** 22px full-round clay chip; variants tint to celadon/iron/copper/cobalt washes with matching glaze text; the danger variant always rides the cross-hatch pattern.
- **Status chip:** mono 13px, tabular numerals, painted on a white plaque with a hairline — the signed counter "↑2 ↓1 12개 파일".
- **Hash chip:** 12px mono on clay with a hairline — short SHAs and addresses.

### Cards / Containers
- **Corner Style:** 12px (`md`), 16px for the CTA and modal.
- **Background:** white plaque on the porcelain canvas, 1px engraved hairline, plaque-rest shadow; 24px internal padding.

### Inputs / Fields
- **Style:** white plaque, 1px strong hairline, 8px radius, 40px height, inset top light.
- **Focus:** border shifts to cobalt + a 3px 14%-cobalt ring; the caret is cobalt.
- **Placeholders** use ink-muted (4.5:1, never faint); checkboxes/radios are `accent-color` cobalt.

### Navigation
- 8px-radius rows on the clay field; hover is a 55%-white glaze wash; the active item is a white plaque with hairline ring + plaque-rest shadow + a carved 3px cobalt tick at its left, icon re-tinted cobalt, weight 600.
- The account chip sits on the bottom hairline; the team unread badge is a cobalt pill.

### Tabs
- A clay plate (10px radius, 1px hairline) holding 30px segments; the active segment is a white plaque with hairline ring and shadow. Present for 작업 / 병합 / 설정 and the 팀 panel's 프로젝트 / 만들기 / 참여하기 / 알림.

### Toast
- Dark fired porcelain (#2a2d34) with a white 10% hairline, kiln-float shadow; enters with a blur-to-sharp settle 240ms expo-ease; a 20px tinted icon tile carries its kind (celadon ok / copper error / cobalt info).

### Signature: the Cobalt CTA Plaque
The Home view's project-add card is the app's one saturated statement: a 165° cobalt gradient (#32518f → #21396b), 16px radius, glaze top-light and radial sheen, white 700 title, white 72%-alpha description, and the inverse white plaque button. It is the only full-cobalt block in the system — the One Cobalt Rule in action.

### Banners
Porcelain slips with a 1px tinted left edge (never ≥2px), tinted ground, and a tinted icon that inherits the edge color; titles and bodies stay ink.

## Do's and Don'ts

### Do:
- **Do** keep cobalt scarce — one saturated block per screen; elsewhere it is mark-sized (tick, ring, tile, icon).
- **Do** engrave state: pair every hue with a pattern or mark (cross-hatch conflict, cobalt tick active, hatch for staged).
- **Do** set counters, SHAs, ports, and paths in mono with tabular numerals.
- **Do** use porcelain-white plaques over the warm canvas, separated by 1px hairlines and soft offset+blur shadows.
- **Do** let glaze read as light: `inset 0 1px 0 rgba(255,255,255,.14–.16)` top highlights on cobalt and plaque buttons.

### Don't:
- **Don't** use pure black ink, pure white flat canvases, or the old pink accent (#ff385c family) anywhere.
- **Don't** put a colored edge thicker than 1px on cards, banners, or list rows.
- **Don't** use gradient text, hard zero-blur offset shadows, or glow/halo effects.
- **Don't** let a status exist in color alone — every state badge has its pattern or mark.
- **Don't** use emoji or unicode glyphs where the icon system applies; icons are drawn Lucide strokes in one weight (2px, round caps).
- **Don't** add kickers/eyebrows above headings, section numbers, or hero-metric stat cards — the world's information is ledger-dense, not poster-dense.