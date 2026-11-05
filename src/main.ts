// SPDX-License-Identifier: MPL-2.0
import { mount } from 'svelte';
import { Menu } from '@tauri-apps/api/menu';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import './app.css';
import App from './App.svelte';

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
    void showCopyMenu(selected);
    return;
  }

  event.preventDefault();
});

async function showCopyMenu(text: string) {
  const label = navigator.language.startsWith('fr') ? 'Copier' : 'Copy';
  const menu = await Menu.new({
    items: [{ id: 'copy', text: label, action: () => void writeText(text) }],
  });
  await menu.popup();
}

export default mount(App, { target: document.getElementById('app')! });
