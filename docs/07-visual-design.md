# 🎨 Visual Design

How Soundfolk looks: colors, type, icons, spacing and motion. [06-ui-principles.md](06-ui-principles.md) says what
the interface shows and where; this document says how it is drawn. The tokens live in `src/app.css`; components use
them through their CSS Modules and never hard-code a color.

The direction is a quiet tuner: neutral surfaces, one warm dial light, nothing that competes with the sound.

---

## 1. Themes
* **System, Light or Dark**, chosen in Settings › Appearance. *System* follows the desktop through
  `prefers-color-scheme`.
* The choice is stored in the webview's local storage. `src/lib/theme.ts` always sets `data-theme` on `<html>` to
  `light` or `dark` before the first render; for *System* it reads `prefers-color-scheme` and follows its changes.
* Dark is the default in `:root`; the light values live once, under `:root[data-theme='light']`. The CSS has no
  `prefers-color-scheme` query of its own.
* Every color is a `--color-*` token defined for both themes. A new token is added to both blocks.

## 2. Color
* **One neutral ramp per theme** for surfaces, from `--color-bg` to `--color-surface-control`. A raised element
  takes the next step of the ramp rather than a shadow or a tint.
* **Hairline borders:** the primary text color at low opacity (`--color-border` … `--color-border-hover`), so they
  read the same on every surface.
* **Three text roles:** `--color-text` (names, titles), `--color-text-soft` / `--color-text-muted` (secondary
  information), `--color-text-subtle` (hints, timestamps). `--color-disabled` only for disabled controls.
* **Amber is rare.** `--color-accent` (`#f0a028`, the tuner's dial light) appears only on:
  * the play button;
  * the selected tab;
  * the station on air (a small lamp before its name);
  * the highest value of each chart.

  Everything else — chips, links, toggles, secondary buttons — stays neutral. When amber is used as text or as an
  icon on a surface, use `--color-accent-text` (darker in the light theme for contrast).
* **Status colors** (success, info, warning, error, danger) are toned to sit inside the ramp: a tinted background, a
  border and a text color per status, never a saturated fill except for a destructive confirm button.
* `--color-like` is reserved for the heart of a liked track.

## 3. Type
* **System font stack** (`system-ui`, `-apple-system`, `Segoe UI`): the app looks native on each desktop and ships
  no font.
* **Two weights:** 400 for text, 500 for emphasis (names, headings, the selected item). No bold, no uppercase
  tracked labels.
* **A short size scale** in `rem`: `0.6875`, `0.75`, `0.8125`, `0.875`, `1`, `1.125`, `1.25`. `0.8125rem` and
  `0.75rem` carry most of the interface; reach for a new size only if none of these fits.
* **Tabular figures** everywhere (`font-variant-numeric: tabular-nums` on `:root`), so counts, durations and
  bitrates line up.

## 4. Icons
* **Line icons only**, from `src/lib/components/Icon.svelte`: Lucide paths inlined (ISC license), no icon package.
* One stroke (1.5) and a default size of 16 px; `filled` fills the shape for an active state (a starred station, a
  liked track).
* Icons are decorative (`aria-hidden`): the button around them carries the label. No emoji or text glyphs (`★`,
  `×`, `⋯`) as icons in the interface.
* A button holding an icon centers it and spaces it from a label next to it (`:where(button:has(> svg))` in
  `app.css`).
* To add an icon: add its name to `IconName` and its Lucide path to the component.

## 5. Spacing and shape
* **Spacing on a 4 px grid** (`0.25rem` steps): paddings, gaps and margins are multiples of 4 px.
* **Radii on a scale:** `0.375rem` (small controls, chips), `0.5rem` (rows, buttons, inputs), `0.75rem` (cards,
  sheets, tiles); `999px` for pills and `50%` for round buttons.
* Tiles in a grid have equal heights (Discover).

## 6. Long text
* **Track titles** (the player, History) wrap to **two lines** before the ellipsis: "Artist - Title" rarely fits one
  line of a narrow window.
* Station names, list names and secondary lines stay on **one line** with an ellipsis.
* Wherever text can be cut, its element carries the full text in `title`.
* No scrolling text (marquee): it loops, which §7 rules out.

## 7. Motion
* One easing curve, `--ease`, and short durations (`--duration-fast`, 150 ms) for hover and state changes.
* Motion confirms an action; it never loops or pulses for attention.
* Every animation has a `prefers-reduced-motion: reduce` fallback that removes it.

## 8. Choices against common requests
Feedback on other music and radio players (Play Store, Reddit, GitHub; a synthesis without sources, so hypotheses)
asks for a few things Soundfolk deliberately does not do:
* **A pure black dark theme.** The dark background stays a near-black grey (`#121213`): it limits the halo of light
  text on black and leaves room for a ramp of raised surfaces. A `data-theme='black'` variant can come with the mobile
  clients, where OLED screens make it worthwhile.
* **Bitrate and codec badges on rows** ("128k MP3", "FLAC"). Lists keep to the human badges of
  [06-ui-principles.md](06-ui-principles.md) §5 (*HD*, *ad-free*…); technical details stay in the station sheet.
* **Soft shadows under cover art.** A raised element takes the next step of the ramp (§2), not a shadow.

The same feedback confirms choices already made: no promotional content, the play button as the single focal point,
covers with 8 px corners, one-pixel low-opacity borders, filters as chips on the results.

## 9. Checklist for a new screen or component
- [ ] Colors only through `--color-*` tokens, tried in both themes.
- [ ] No amber outside the four uses of §2.
- [ ] Text that can be cut has its full text in `title`; track titles get two lines (§6).
- [ ] Sizes, weights and radii from the scales above.
- [ ] Icons from `Icon.svelte`, with a label on the button.
- [ ] Hover and focus visible (`--color-focus`), reduced motion respected.
