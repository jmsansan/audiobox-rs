---
name: audiobox
description: "Signal score: readable audio, precise controls and Rust code."
colors:
  paper: "#edf1fb"
  ink: "#202a50"
  muted: "#4e5c83"
  accent: "#6841cb"
  accent-dark: "#4e2ea1"
  lavender: "#e5dcfa"
  blue: "#dae4fb"
  rule: "#bac6e3"
  white: "#fafbff"
  code: "#18213e"
  danger: "#a32c42"
  code-text: "#eef1ff"
  code-rule: "#394464"
  code-muted: "#b6c3e5"
  syntax-keyword: "#bfafff"
  syntax-string: "#acd9be"
  syntax-comment: "#a4b0d0"
  syntax-number: "#ffd391"
  syntax-type: "#a8ccff"
  signal-grid: "#9aa9d0"
  segment-bed: "#c5d2f1"
typography:
  display:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "clamp(3.4rem, 6.6vw, 6rem)"
    fontWeight: 800
    lineHeight: 1.08
    letterSpacing: "-.035em"
  headline:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "clamp(2.15rem, 4vw, 3.6rem)"
    fontWeight: 750
    lineHeight: 1.08
    letterSpacing: "-.035em"
  title:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "1.35rem"
    fontWeight: 750
    lineHeight: 1.08
    letterSpacing: "-.035em"
  body:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.65
  lead:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 400
    lineHeight: 1.7
  label:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 650
    lineHeight: 1.65
  button:
    fontFamily: "Manrope, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 700
    lineHeight: 1.4
  code:
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.85
  measurement:
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.65
rounded:
  inline-code: "3px"
  segment: "5px"
  field: "6px"
  compact: "7px"
  button: "8px"
  panel: "12px"
  circular: "50%"
spacing:
  gap-small: "9px"
  gap-control: "12px"
  inset-mobile: "18px"
  inset-code: "22px"
  gap-navigation: "30px"
  section-mobile: "70px"
  section-desktop: "105px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.white}"
    typography: "{typography.button}"
    rounded: "{rounded.button}"
    padding: "13px 21px"
  button-primary-hover:
    backgroundColor: "{colors.accent-dark}"
  button-default:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.white}"
    typography: "{typography.button}"
    rounded: "{rounded.button}"
    padding: "13px 21px"
  button-secondary:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.button}"
    rounded: "{rounded.button}"
    padding: "13px 21px"
  button-secondary-hover:
    backgroundColor: "{colors.blue}"
  input-number:
    backgroundColor: "{colors.white}"
    textColor: "{colors.ink}"
    rounded: "{rounded.field}"
    padding: "8px 10px"
  signal-original:
    backgroundColor: "{colors.blue}"
    textColor: "{colors.ink}"
    rounded: "{rounded.panel}"
    padding: "20px 24px"
  signal-processed:
    backgroundColor: "{colors.lavender}"
    textColor: "{colors.ink}"
    rounded: "{rounded.panel}"
    padding: "20px 24px"
  code-panel:
    backgroundColor: "{colors.code}"
    textColor: "{colors.code-text}"
    rounded: "{rounded.panel}"
    typography: "{typography.code}"
  method-link:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.segment}"
    padding: "6px 9px"
  play-button:
    backgroundColor: "{colors.ink}"
    textColor: "white"
    rounded: "{rounded.circular}"
    width: "42px"
    height: "42px"
---
# Design System: audiobox

## Overview

**Creative North Star: "Signal score"**

Signal score makes audio processing readable through pale score-paper fields, deep navy ink and violet output signals. Manrope carries headings and interface copy; monospace distinguishes runnable code and measured values.

The interface stays flat and precise. Live waveforms, explicit playback controls and Rust snippets supply the visual identity; authored SVG marks provide small supporting geometry. The shared system covers the presentation, sandbox and hand-written API guide, not generated rustdoc.

**Key Characteristics:**
- Pale blue and lavender fields with navy ink.
- Manrope headings and compact native controls.
- Monospace code and tabular audio measurements.
- Data-driven waveforms; flat surfaces without shadows.

## Colors

Cool paper and navy ink support a violet processing accent; syntax colors stay inside dark code panels. Exact values are in the frontmatter.

### Primary
- **Signal Violet** (`accent`): processed waveforms, selected operations, primary actions, active navigation and focus. `accent-dark` supplies button hover.

### Secondary
- **Lavender Paper** (`lavender`): processed audio lanes, format section field and documentation callouts.
- **Blue Paper** (`blue`): original audio lanes, homepage score and inline code backgrounds.

### Neutral
- **Score Paper** (`paper`): page background.
- **Navy Ink** (`ink`): headings, default controls and original waveforms.
- **Muted Ink** (`muted`): explanatory text, metadata and measurements.
- **Rule Blue** (`rule`): navigation, tables and control dividers.
- **Near White** (`white`): field backgrounds and button text.
- **Code Navy** (`code`): code panels, with `code-text`, `code-muted` and `code-rule` for their readable content and toolbar.
- **Signal Grid** (`signal-grid`): waveform reference lines. `segment-bed` groups the two audio states.

Error text uses `danger`; syntax keyword, string, comment, number and type tokens are restricted to code highlighting.

**The Signal Identity Rule.** Use navy for original waveforms and violet for processed waveforms; the drawing comes from actual audio samples.

## Typography

**Display and Body Font:** self-hosted variable Manrope, with system-ui and sans-serif fallbacks. The shipped font covers weights 200–800 and uses swap loading.
**Label/Mono Font:** the native monospace stack recorded in the frontmatter.

### Hierarchy
- **Display:** the frontmatter display role is the desktop homepage heading. At the medium breakpoint it is 4.8rem; on mobile it uses `clamp(3.5rem, 12vw, 5.2rem)`. Secondary page headings use smaller clamps; mobile sandbox and guide headings are 2.9rem and 3.1rem.
- **Headline:** large section headings; the guide uses 32px desktop and 28px mobile.
- **Title:** smaller headings; guide subheadings use 21px and sandbox control headings 19px.
- **Body:** base page copy, with a 70ch general maximum; guide paragraphs use 15px desktop and 14px mobile.
- **Lead:** homepage introduction, constrained to 39ch; guide lead uses an 18px size and a 60ch maximum.
- **Label and Button:** compact interface text varies from 12–14px and 650–750 weight. Visible small labels and measurements are at least 12px.
- **Code and Measurement:** code blocks use the code role; mobile code reduces to 12px. Times and audio measurements use 12px tabular numerals.

**The Two Voices Rule.** Use Manrope for display and interface copy; reserve monospace for code, API identifiers and audio measurements.

## Layout

The shared centered wrapper is capped at 1200px with 40px side gutters. Above 1550px, the cap becomes 1300px with 60px gutters; at 1000px and below gutters become 24px, and at 720px and below 18px. The header is 88px high, reducing to 75px on mobile.

Desktop layouts pair asymmetric columns: homepage introduction 1.35:1, examples .7:1.3, formats .75:1.25. The sandbox pairs a flexible audio area with a 300px controls column (260px at the medium breakpoint); the guide pairs a 200px sticky sidebar with an article up to 830px wide (170px sidebar at medium). Desktop section spacing is 105px; mobile spacing is 70px. This is a varied spacing vocabulary, not a fixed base-unit grid.

At 720px the presentation columns stack, examples become a horizontally scrollable tab row, sandbox controls form two columns below the audio, and the Rust output follows controls across the full grid width. The guide sidebar becomes a wrapping navigation row. Mobile API tables stack method and description within each row; wider format tables retain local horizontal scrolling and an explicit scroll hint. Code panels and their grid wrapper constrain their width and scroll internally. The segmented state control resists shrinking and keeps its labels on one line.

## Elevation & Depth

No box shadows are shipped. Blue, lavender and dark code fields establish grouping; thin rules divide text and controls. Focus is a three-pixel violet outline with a four-pixel offset; the upload control uses a three-pixel offset. Buttons shift up one pixel on hover, with background and transform transitioning over .18s ease. Reduced-motion preferences remove transitions and smooth scrolling and hide the animated playhead.

**The Flat Field Rule.** Separate surfaces with color fields and thin rules; resting surfaces have no box shadows.

## Shapes

Large score, lane and code panels use the shared 12px corner. Buttons and callouts use 8px; file/search controls use 7px; native numeric and select fields use 6px; segment selections and API links use 5px; inline code uses 3px. Playback controls are circular. Navigation and control rows remain unboxed, separated by thin horizontal rules.

Authored SVG icons use rounded stroke ends and joins, normally 22px with 1.8px strokes. Size and stroke are adapted for the brand mark and compact controls. Waveform geometry is rendered on canvas from sample envelopes.

## Components

### Buttons
Precise, compact actions. Primary uses violet; default uses navy; secondary is transparent with a thin rule-colored border. All share the button token; medium-screen padding reduces to 12px 16px. Hover uses darker violet, except secondary uses blue paper; shared focus remains visible. Small buttons use 9px 15px padding and 13px text. Disabled buttons show .55 opacity and a wait cursor.

### Inputs / Fields
Native selects and number inputs use near-white fill, thin rule borders, 6px corners and 8px 10px padding. Labels stay visible above or alongside the control. Range and checkbox accent colors are violet. Trim fields use native required and number-bound validation; runtime status errors use danger text with bold weight. The docs search field uses 7px corners and 12px text.

### Navigation
Manrope, bold, compact and text-led. Main links use 14px desktop and 12px mobile; active and hovered links turn violet. The repository link disappears at the mobile breakpoint. The guide sidebar is sticky on desktop and becomes a wrapping row on mobile.

### API Method Links
Outlined, compact monospace links with 5px corners, 6px 9px padding and blue-paper hover. They name concrete API destinations rather than decorative tags.

### Signal Score / Audio Lanes
Flat score fields frame actual source and processed waveforms. Homepage signal height is 144px (126px mobile); sandbox lanes use 110px signals. Original and processed lanes use blue and lavender respectively. Circular play buttons are 42px in the desktop score, 36px in the mobile score and 35px in sandbox lanes. Times and measurements use monospace; the thin moving playhead follows playback. Reduced motion hides the playhead.

### Segmented Signal State
A compact blue bed surrounds two transparent buttons; the pressed button uses near-white fill and violet text. The group and selection corners are 7px and 5px. The state is exposed through `aria-pressed`, with shared keyboard focus.

### Code Panels
Dark, clipped containers with 12px corners; a thin divider separates a monospace toolbar from scrollable Rust code. Toolbar padding is 15px 22px and code padding is 25px 22px; mobile reduces these to 13px 17px and 20px 17px. Copy is a quiet text action. Syntax colors provide readable distinctions inside the panel.

## Do's and Don'ts

### Do:
- **Do** preserve the original-to-processed color relationship and draw waveforms from real audio.
- **Do** keep labels attached to their native controls and measurements readable with tabular numbers.
- **Do** retain the shared focus outline and honor reduced-motion preferences.
- **Do** constrain grid children and allow code and wide format tables to scroll locally.

### Don't:
- **Don't** substitute decorative waveform artwork for the audio data.
- **Don't** use monospace for display headings.
- **Don't** add raised shadows to the flat score fields.
