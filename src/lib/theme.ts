// SPDX-License-Identifier: MPL-2.0
// Light or dark theme (Settings › Appearance). `data-theme` on <html> is always set to "light" or
// "dark", which app.css reads; "system" resolves it from the desktop's `prefers-color-scheme` and
// follows its changes. Resolving here keeps a single set of light values in app.css. Kept in the
// webview's local storage: it only concerns this window, and it is applied before the first
// render, with no round trip to the backend.

export type Theme = 'system' | 'light' | 'dark';

const KEY = 'soundfolk.theme';
const systemDark = window.matchMedia('(prefers-color-scheme: dark)');

let current: Theme = 'system';

function resolve(theme: Theme): 'light' | 'dark' {
  if (theme !== 'system') return theme;
  return systemDark.matches ? 'dark' : 'light';
}

systemDark.addEventListener('change', () => {
  if (current === 'system') applyTheme('system');
});

export function savedTheme(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === 'light' || value === 'dark' ? value : 'system';
  } catch {
    return 'system';
  }
}

export function applyTheme(theme: Theme) {
  current = theme;
  document.documentElement.dataset.theme = resolve(theme);
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
