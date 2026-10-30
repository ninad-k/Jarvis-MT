<script lang="ts">
	import '@fontsource-variable/inter';
	import '@fontsource-variable/jetbrains-mono';
	import './layout.css';
	import { onMount } from 'svelte';
	import favicon from '#lib/assets/favicon.svg';
	import Toasts from '#lib/components/Toasts.svelte';
	import { pwa } from '#lib/state/pwa.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { applyTheme } from '#lib/theme.ts';
	// Importing the auth store registers the "any 401 goes to /login" handler for every route.
	import '#lib/state/auth.svelte.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	onMount(() => pwa.attach());

	// The active persona's colours go onto <html> as CSS custom properties; layout.css animates them.
	// The first application (the persona remembered from last time) is instant: no blue-to-gold flash.
	let themed = false;
	$effect(() => {
		const el = document.documentElement;
		const first = !themed;
		if (first) el.style.transition = 'none';
		applyTheme(el, persona.theme, persona.activeId ?? undefined);
		if (first) {
			themed = true;
			void el.offsetWidth; // commit the new values before transitions come back
			requestAnimationFrame(() => el.style.removeProperty('transition'));
		}
	});
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>Arc MT</title>
</svelte:head>

{@render children()}
<Toasts />
