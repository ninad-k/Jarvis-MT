<script lang="ts">
	import { goto } from '$app/navigation';
	import Menu, { type MenuItem } from './Menu.svelte';
	import { auth } from '#lib/state/auth.svelte.ts';
	import { backend } from '#lib/state/hud.svelte.ts';
	import { pwa } from '#lib/state/pwa.svelte.ts';
	import { speech } from '#lib/state/speech.svelte.ts';

	const items = $derived.by<MenuItem[]>(() => {
		const out: MenuItem[] = [];
		if (auth.username) out.push({ id: 'user', label: `Signed in as ${auth.username}`, disabled: true });
		out.push({ id: 'settings', label: 'Settings', divider: out.length > 0 });
		out.push({ id: 'audit', label: 'Audit log' });
		if (backend.ttsAvailable)
			out.push({ id: 'aloud', label: 'Read replies aloud', checked: speech.readAloud });
		if (pwa.canInstall) out.push({ id: 'install', label: 'Install app' });
		else if (pwa.iosHint)
			out.push({ id: 'ios', label: 'Install: Share, then Add to Home Screen', disabled: true });
		if (auth.enabled && auth.username) out.push({ id: 'logout', label: 'Log out', divider: true, danger: true });
		return out;
	});

	function onselect(id: string) {
		if (id === 'settings') void goto('/settings');
		else if (id === 'audit') void goto('/audit');
		else if (id === 'aloud') speech.setReadAloud(!speech.readAloud);
		else if (id === 'install') void pwa.install();
		else if (id === 'logout') void auth.logout();
	}
</script>

<Menu
	label={auth.username ? `Menu for ${auth.username}` : 'Menu'}
	{items}
	{onselect}
	class="num flex h-7 items-center gap-1.5 border border-line bg-panel/70 px-2.5 text-2xs text-mute transition-colors hover:border-accent hover:text-ink"
>
	{#snippet trigger()}
		<svg viewBox="0 0 24 24" class="size-3.5" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
			<circle cx="12" cy="8" r="3.5" />
			<path d="M5 20c.8-3.6 3.5-5.5 7-5.5s6.2 1.9 7 5.5" stroke-linecap="round" />
		</svg>
		{#if auth.username}<span class="max-w-24 truncate max-md:hidden">{auth.username}</span>{/if}
		<span aria-hidden="true">▾</span>
	{/snippet}
</Menu>
