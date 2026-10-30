<script lang="ts">
	import { onMount } from 'svelte';
	import type { OrbScene } from '#lib/orb/OrbScene.ts';
	import { orb } from '#lib/state/orb.svelte.ts';
	import { backend, status } from '#lib/state/hud.svelte.ts';
	import { provider } from '#lib/state/provider.svelte.ts';

	let host = $state<HTMLDivElement>();
	let scene: OrbScene | undefined;
	let fallback = $state(false);

	onMount(() => {
		let disposed = false;
		const el = host;
		if (!el) return;
		// three.js is ~550 kB: load it after first paint so the HUD appears immediately.
		import('#lib/orb/OrbScene.ts')
			.then(({ OrbScene }) => {
				if (disposed) return;
				scene = new OrbScene(el, {
					getAmplitude: () => orb.amplitude,
					lowPower: matchMedia('(max-width: 767px)').matches || navigator.maxTouchPoints > 0,
					onContextLost: () => (fallback = true),
					onContextRestored: () => (fallback = false)
				});
				scene.setState(orb.state);
				scene.start();
				if (import.meta.env.DEV) (window as unknown as { __scene: OrbScene }).__scene = scene;
			})
			.catch((e) => {
				console.warn('Orb: WebGL unavailable, using CSS fallback', e);
				fallback = true;
			});
		if (import.meta.env.DEV) (window as unknown as { __orb: typeof orb }).__orb = orb;
		return () => {
			disposed = true;
			scene?.dispose();
			scene = undefined;
		};
	});

	$effect(() => {
		// Read the state first: `scene` is non-reactive and may be undefined on the first run.
		const next = orb.state;
		scene?.setState(next);
	});

	// ?orb=listening|thinking|speaking|error pins the state in dev builds (visual checks).
	onMount(() => {
		if (!import.meta.env.DEV) return;
		const forced = new URLSearchParams(location.search).get('orb');
		if (forced === 'speaking') orb.speak(600_000);
		else if (forced === 'idle' || forced === 'listening' || forced === 'thinking' || forced === 'error')
			orb.set(forced);
	});

	const label = $derived(
		backend.offline && orb.state === 'idle' ? 'Backend offline' : orb.label
	);
	const llm = $derived(
		status.data?.llm.providers.find((p) => p.name === provider.selected) ?? null
	);
</script>

<div class="relative flex h-full min-h-0 w-full flex-col items-center justify-center overflow-hidden">
	<div bind:this={host} class="absolute inset-0" role="img" aria-label="Jarvis status orb: {label}">
		{#if fallback}
			<div class="grid h-full place-items-center">
				<div
					class="aspect-square h-[46%] rounded-full motion-safe:animate-[fallback-breathe_5s_ease-in-out_infinite]"
					style="background:radial-gradient(circle at 50% 42%, #c9f8ff 0%, #22d3ee 28%, #0a3f86 62%, transparent 72%);box-shadow:0 0 90px 24px rgb(34 211 238 / .35)"
				></div>
			</div>
		{/if}
	</div>

	<!-- HUD readouts: real data only. -->
	<dl class="num pointer-events-none absolute top-2 left-3 hidden text-2xs leading-relaxed tracking-wider text-faint uppercase sm:block">
		<div class="flex gap-2"><dt>Core</dt><dd class="text-mute">{orb.state}</dd></div>
		<div class="flex gap-2"><dt>Link</dt><dd class="text-mute">{backend.offline ? 'down' : status.data ? (status.data.bridge.connected ? 'up' : 'down') : '…'}</dd></div>
	</dl>
	<dl class="num pointer-events-none absolute top-2 right-3 hidden text-right text-2xs leading-relaxed tracking-wider text-faint uppercase sm:block">
		<div class="flex justify-end gap-2"><dt>LLM</dt><dd class="text-mute">{llm?.name ?? '—'}</dd></div>
		<div class="flex justify-end gap-2"><dt>Model</dt><dd class="text-mute normal-case">{llm?.model ?? '—'}</dd></div>
	</dl>

	<p
		class="relative z-10 mt-auto mb-2 flex items-center gap-2 text-xs tracking-[0.22em] uppercase transition-colors sm:mb-4 sm:text-sm {orb.state ===
		'error'
			? 'text-down'
			: orb.state === 'thinking'
				? 'text-violet'
				: 'text-accent'}"
		role="status"
	>
		<span class="size-1.5 rounded-full bg-current motion-safe:animate-[pulse-dot_1.6s_ease-in-out_infinite]" aria-hidden="true"></span>
		{label}
	</p>
</div>
