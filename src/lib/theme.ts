// SPDX-License-Identifier: MPL-2.0
// Light or dark theme (Settings › Appearance). "system" follows the desktop's preference through
// `prefers-color-scheme`; the other two set `data-theme` on <html>, which app.css reads. Kept in
// the webview's local storage: it only concerns this window, and it is applied before the first
// render, with no round trip to the backend.

export type Theme = 'system' | 'light' | 'dark';

const KEY = 'soundfolk.theme';

export function savedTheme(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === 'system') delete root.dataset.theme;
  else root.dataset.theme = theme;
}

export function setTheme(theme: Theme) {
  applyTheme(theme);
  try {
    if (theme === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, theme);
  } catch {
    // Storage unavailable: the choice holds until the window closes.
  }
}
