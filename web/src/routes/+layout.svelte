<script lang="ts">
	import '@fontsource-variable/inter';
	import '@fontsource-variable/jetbrains-mono';
	import './layout.css';
	import { onMount } from 'svelte';
	import favicon from '#lib/assets/favicon.svg';
	import Toasts from '#lib/components/Toasts.svelte';
	import { pwa } from '#lib/state/pwa.svelte.ts';
	// Importing the auth store registers the "any 401 goes to /login" handler for every route.
	import '#lib/state/auth.svelte.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	onMount(() => pwa.attach());
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>Jarvis MT</title>
</svelte:head>

{@render children()}
<Toasts />
