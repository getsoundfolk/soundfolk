# Accessibility

Soundfolk should be usable by everyone who wants to listen to the radio, with a keyboard, a screen reader or
reduced motion. This page says what is done, what has not been checked, and how to tell us about a barrier.

## What Soundfolk does

- **Keyboard.** Controls are reached with Tab and show a visible focus ring. Tabs and option groups follow
  the WAI-ARIA patterns: the arrow keys, Home and End move between them, and Tab leaves the group. Shortcuts in the
  main window: Space plays or pauses, M mutes, Ctrl+F or Ctrl+K go to the search, Up and Down move between the
  stations of a list. A shortcut never takes a key from a text field.
- **Reordering without a mouse.** Favorites and ordered lists can be moved with the keyboard, and each move is
  announced to screen readers.
- **Screen readers.** Buttons with an icon carry a text label, and the icon itself is hidden from assistive
  technology. The player's state (Playing, Paused, Stopped) is given to screen readers even where it is not written on
  screen. Confirmations, such as a copied title, are announced as status messages. Text cut with an ellipsis keeps its
  full content in a tooltip.
- **Contrast.** Text and interface colors are chosen for WCAG 2.1 AA contrast in the light and the dark theme; the brand
  orange is not used for text or buttons because it falls short
  ([docs/07-visual-design.md](docs/07-visual-design.md) §2).
- **Motion.** Every animation, in CSS or in JavaScript, is removed when the system asks for reduced motion. No
  scrolling text.
- **Theme.** Light and dark, following the system or chosen in the settings.
- **Without the window.** Media keys, the tray menu and track notifications let you play, pause, mute and change
  station without opening the window.

## Known limitations

- **Not tested with a screen reader yet.** The labels and roles above are written for screen readers, but nobody has
  gone through the app with Orca, NVDA or VoiceOver. Expect rough edges, and please tell us about them.
- **No high-contrast mode.** The system's high-contrast and forced-colors settings are not taken into account.
- **English only.** The interface is not translated.
- **Desktop only.** There is no mobile app yet.

## Reporting a barrier

Open a [bug report](https://github.com/getsoundfolk/soundfolk/issues/new?template=bug.yml) and say that it is about
accessibility. Tell us what you were trying to do, what got in the way, and the assistive technology you use (screen reader, magnifier, keyboard only…) with its version.
