// SPDX-License-Identifier: MPL-2.0
import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';

export default mount(App, { target: document.getElementById('app')! });
