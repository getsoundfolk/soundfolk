// SPDX-License-Identifier: MPL-2.0
import { mount } from 'svelte';
import { Menu } from '@tauri-apps/api/menu';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import './app.css';
import { getCurrentWindow } from '@tauri-apps/api/window';
import App from './App.svelte';
import MiniPlayer from './lib/components/MiniPlayer.svelte';
import { applyTheme, savedTheme } from './lib/theme';

applyTheme(savedTheme());

// The webview's own right-click menu (Back, Reload, Inspect, Copy Link with
// Highlight…) makes no sense in the app. Over selected text it is replaced by
// a menu holding only Copy; text fields keep the native one for cut/copy/paste.
// In dev builds the inspector stays on Ctrl+Shift+I.
document.addEventListener('contextmenu', (event) => {
  const target = event.target as HTMLElement | null;
  if (target?.closest('input, textarea, [contenteditable="true"]')) return;

  const selected = window.getSelection()?.toString() ?? '';
  if (selected.trim()) {
    event.preventDefault();
    // A menu that cannot open leaves the selection as it is: nothing to tell.
    showCopyMenu(selected).catch((e) => console.warn('copy menu not shown', e));
    return;
  }

  event.preventDefault();
});

async function showCopyMenu(text: string) {
  const label = navigator.language.startsWith('fr') ? 'Copier' : 'Copy';
  const menu = await Menu.new({
    items: [
      {
        id: 'copy',
        text: label,
        action: () => writeText(text).catch((e) => console.warn('text not copied', e)),
      },
    ],
  });
  await menu.popup();
}

// The mini player's window runs this same front end (src-tauri/src/mini.rs): its label says
// which of the two to show.
const mini = getCurrentWindow().label === 'mini';
if (mini) document.documentElement.dataset.window = 'mini';

export default mount(mini ? MiniPlayer : App, { target: document.getElementById('app')! });
