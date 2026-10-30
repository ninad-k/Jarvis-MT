<script lang="ts">
	import { onMount } from 'svelte';
	import { beforeNavigate } from '$app/navigation';
	import { api, type Status } from '#lib/api.ts';
	import AliasesSection from '#lib/components/settings/AliasesSection.svelte';
	import LimitsSection from '#lib/components/settings/LimitsSection.svelte';
	import ModelSection from '#lib/components/settings/ModelSection.svelte';
	import PersonaSection from '#lib/components/settings/PersonaSection.svelte';
	import VoiceSection from '#lib/components/settings/VoiceSection.svelte';
	import WatchlistSection from '#lib/components/settings/WatchlistSection.svelte';
	import ConfirmModal from '#lib/components/ConfirmModal.svelte';
	import { DirtyRegistry } from '#lib/state/draft.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { provideDirtyRegistry } from '#lib/state/settings-context.ts';
	import { settings } from '#lib/state/settings.svelte.ts';

	const dirty = new DirtyRegistry();
	provideDirtyRegistry(dirty);

	/** What /api/status says about voice, models and login. Read once: this page has no pollers. */
	let status = $state<Status | null>(null);

	const NAV = [
		{ id: 'persona', label: 'Persona' },
		{ id: 'voice', label: 'Voice' },
		{ id: 'model', label: 'Model' },
		{ id: 'watchlist', label: 'Watchlist' },
		{ id: 'aliases', label: 'Aliases' },
		{ id: 'limits', label: 'Limits' }
	];

	onMount(() => {
		void persona.ensureLoaded();
		api
			.status()
			.then((s) => (status = s))
			.catch(() => {});
		const warn = (e: BeforeUnloadEvent) => {
			if (dirty.any) e.preventDefault();
		};
		window.addEventListener('beforeunload', warn);
		return () => window.removeEventListener('beforeunload', warn);
	});

	// Leaving with unsaved edits asks first (in-app links; closing the tab uses beforeunload above).
	beforeNavigate((nav) => {
		if (!dirty.any) return;
		if (nav.willUnload) {
			nav.cancel();
			return;
		}
		if (!confirm('You have unsaved changes on this page. Leave without saving?')) nav.cancel();
	});
</script>

<svelte:head><title>Settings · Arc MT</title></svelte:head>

<!-- A confirmation can open here too (a spoken request on the main screen is still pending). -->
<ConfirmModal />

<div class="flex h-dvh flex-col">
	<header
		class="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b border-line bg-deck/80 px-4 pt-[max(0.5rem,env(safe-area-inset-top))] pb-2 backdrop-blur"
	>
		<a href="/" class="num text-xs text-mute hover:text-ink" data-testid="back-to-hud">← HUD</a>
		<h1 class="font-mono text-sm font-semibold tracking-[0.2em] uppercase">Settings</h1>
		<nav aria-label="Settings sections" class="ml-auto flex flex-wrap gap-x-3 gap-y-1 text-2xs text-mute">
			{#each NAV as n (n.id)}
				<a href="#{n.id}" class="uppercase tracking-widest hover:text-ink">{n.label}</a>
			{/each}
		</nav>
	</header>

	<main class="min-h-0 flex-1 overflow-y-auto p-4 pb-[max(1rem,env(safe-area-inset-bottom))]">
		<div class="mx-auto max-w-3xl space-y-4">
			{#if settings.load === 'loading' || settings.load === 'idle'}
				<div class="space-y-3" aria-busy="true" aria-label="Loading settings">
					{#each [0, 1, 2] as i (i)}<div class="skeleton h-32"></div>{/each}
				</div>
			{:else if settings.load === 'unsupported'}
				<div class="hud p-5 text-sm" role="status">
					<p class="font-medium text-ink">Settings are not available on this server.</p>
					<p class="mt-1 text-xs text-mute">
						This server is older than the web app and has no personas or per-user settings. Update the server to use them.
					</p>
				</div>
			{:else if settings.load === 'error' && !settings.data}
				<div class="hud flex items-center justify-between gap-3 p-5 text-sm text-down" role="alert">
					<span><span aria-hidden="true">△ </span>Could not load your settings. {settings.error ?? ''}</span>
					<button type="button" class="btn" onclick={() => void settings.refresh(true)}>Retry</button>
				</div>
			{:else if settings.data}
				<PersonaSection ttsAvailable={status?.voice?.tts === true} />
				<VoiceSection ttsAvailable={status?.voice?.tts === true} sttAvailable={status === null ? true : status.voice?.stt === true} />
				<ModelSection llm={status?.llm ?? null} />
				<WatchlistSection />
				<AliasesSection />
				<LimitsSection authEnabled={status ? (status.auth?.enabled ?? null) : null} />
			{/if}
		</div>
	</main>
</div>
