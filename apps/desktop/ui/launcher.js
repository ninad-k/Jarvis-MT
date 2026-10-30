// Launcher page: reads the saved backend URL, connects, and shows errors.
// IPC goes through the same call @tauri-apps/api's invoke() makes; the page
// is plain HTML, so it avoids a bundler. Only two commands are permitted
// (capabilities/default.json) and only for this bundled page.
'use strict';

const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);

const form = document.getElementById('form');
const input = document.getElementById('url');
const button = document.getElementById('connect');
const statusEl = document.getElementById('status');
const errorEl = document.getElementById('error');

function showError(message) {
	errorEl.textContent = message;
	errorEl.hidden = !message;
}

async function connect(url) {
	showError('');
	button.disabled = true;
	form.hidden = true;
	statusEl.textContent = 'Connecting to ' + url + ' ...';
	try {
		// On success the window navigates away; nothing more to do here.
		await invoke('connect', { url });
	} catch (err) {
		statusEl.textContent = 'Could not open the backend.';
		showError(String(err));
		form.hidden = false;
		button.disabled = false;
		input.focus();
	}
}

async function start() {
	if (!window.__TAURI_INTERNALS__) {
		statusEl.textContent = 'This page only works inside the Jarvis MT desktop app.';
		return;
	}
	const saved = await invoke('get_backend_url');
	input.value = saved;
	if (location.hash === '#settings') {
		statusEl.textContent = 'Backend settings';
		form.hidden = false;
		input.focus();
		input.select();
	} else {
		connect(saved);
	}
}

form.addEventListener('submit', (event) => {
	event.preventDefault();
	connect(input.value);
});

// The menu entry "Backend settings..." navigates here with #settings.
window.addEventListener('hashchange', () => location.reload());

start();
