<script lang="ts">
	import type { Persona } from '#lib/api.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { speech } from '#lib/state/speech.svelte.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import SettingsSection from './SettingsSection.svelte';

	let { ttsAvailable }: { ttsAvailable: boolean } = $props();

	let feedback = $state<Feedback | null>(null);
	let previewing = $state<string | null>(null);

	const lock = $derived(persona.lockReason);

	async function select(p: Persona) {
		feedback = null;
		const ok = await persona.select(p.id);
		if (ok) feedback = { kind: 'ok', text: `${p.name} is now your assistant. The next reply uses ${p.name}.` };
	}

	async function preview(p: Persona) {
		previewing = p.id;
		try {
			await speech.speak(`Hi, I'm ${p.name}. Say ${p.wake_word}, and I'll be right with you.`, p.id, true);
		} finally {
			previewing = null;
		}
	}

	const previewBlocked = (p: Persona) =>
		!ttsAvailable ? 'Speech output is not available on this server' : !p.voice_available ? `${p.name}'s voice model is missing on the server` : null;
</script>

<SettingsSection id="persona" title="Persona" hideActions {feedback}>
	{#snippet intro()}
		A persona changes the assistant's name, personality, voice, wake phrase and colours. Trading rules, limits and
		confirmations are the same for every persona.
	{/snippet}

	{#if !persona.supported}
		<p class="text-xs text-mute">No personas were found on the server.</p>
	{:else}
		<ul class="grid gap-3 sm:grid-cols-2" aria-label="Personas">
			{#each persona.list as p (p.id)}
				{@const active = p.id === persona.activeId}
				{@const blocked = previewBlocked(p)}
				<li
					class="flex flex-col gap-3 border p-3 {active ? 'border-accent bg-accent/10' : 'border-line bg-panel/60'}"
					data-testid="persona-card"
					data-persona={p.id}
					data-active={active}
				>
					<div class="flex items-start justify-between gap-3">
						<div class="min-w-0">
							<h3 class="text-sm font-semibold text-ink">{p.name}</h3>
							<p class="text-xs text-mute">{p.description}</p>
						</div>
						<span class="flex shrink-0 gap-1" aria-hidden="true" title="Colours: accent, soft accent, orb core, orb glow">
							{#each [p.theme.accent, p.theme.accent_soft, p.theme.orb_core, p.theme.orb_glow] as c, i (i)}
								<span class="size-4 rounded-full border border-white/20" style:background-color={c}></span>
							{/each}
						</span>
					</div>
					<p class="text-xs text-mute">
						Wake phrase <span class="num text-ink">“{p.wake_word}”</span>
					</p>
					<div class="mt-auto flex flex-wrap gap-2">
						<button
							type="button"
							class="btn"
							disabled={blocked !== null || previewing !== null}
							title={blocked ?? undefined}
							onclick={() => void preview(p)}
						>
							{previewing === p.id ? 'Playing…' : 'Preview voice'}
						</button>
						<button
							type="button"
							class="btn {active ? 'btn-primary' : ''}"
							aria-pressed={active}
							disabled={lock !== null && !active}
							title={lock && !active ? lock : undefined}
							onclick={() => (active ? undefined : void select(p))}
						>
							{active ? 'Selected' : `Select ${p.name}`}
						</button>
					</div>
					{#if blocked}
						<p class="text-2xs text-mute">{blocked}</p>
					{/if}
				</li>
			{/each}
		</ul>
		{#if persona.lockReason}
			<p class="text-xs text-warn" role="status"><span aria-hidden="true">△ </span>{persona.lockReason}.</p>
		{/if}
	{/if}
</SettingsSection>
