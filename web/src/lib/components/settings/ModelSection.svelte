<script lang="ts">
	import type { LlmProvider } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import { Draft } from '#lib/state/draft.svelte.ts';
	import { settings } from '#lib/state/settings.svelte.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import SettingsSection from './SettingsSection.svelte';

	let { llm }: { llm: { default: string; providers: LlmProvider[] } | null } = $props();

	// '' stands for "server default" (null on the wire).
	const draft = new Draft<string>(() => settings.data!.llm_provider ?? '');
	let saving = $state(false);
	let feedback = $state<Feedback | null>(null);

	const defaultProvider = $derived(llm?.providers.find((p) => p.name === llm.default));
	const chosen = $derived(llm?.providers.find((p) => p.name === draft.value));

	async function save() {
		saving = true;
		feedback = null;
		try {
			const next = await settings.update({ llm_provider: draft.value === '' ? null : draft.value });
			draft.adopt(next.llm_provider ?? '');
			feedback = {
				kind: 'ok',
				text: next.llm_provider ? `New messages use ${next.llm_provider}.` : 'New messages use the server default.'
			};
		} catch (e) {
			feedback = { kind: 'error', text: describeError(toApiError(e)) };
		} finally {
			saving = false;
		}
	}
</script>

<SettingsSection
	id="model"
	title="Assistant model"
	dirty={draft.dirty}
	{saving}
	{feedback}
	onsave={save}
	onrevert={() => draft.revert()}
>
	{#snippet intro()}
		The language model that writes the assistant's replies. Providers whose API key is missing on the server can't be picked.
	{/snippet}

	{#if llm === null}
		<p class="text-xs text-mute" role="status">Loading the list of models…</p>
	{:else}
		<div class="max-w-md">
			<label for="llm-setting" class="hud-title block">Model provider</label>
			<select id="llm-setting" class="input mt-1" bind:value={draft.value} data-testid="llm-setting">
				<option value="">
					Server default{defaultProvider ? ` (${defaultProvider.name} · ${defaultProvider.model})` : ''}
				</option>
				{#each llm.providers as p (p.name)}
					<option value={p.name} disabled={!p.available}>
						{p.name} · {p.model}{p.available ? '' : ' — API key not set on the server'}
					</option>
				{/each}
			</select>
			{#if chosen}
				<p class="mt-1 text-xs text-mute">
					{chosen.kind} · {chosen.model}
				</p>
			{/if}
		</div>
		{#if llm.providers.some((p) => !p.available)}
			<p class="text-xs text-mute">
				Unavailable: {llm.providers
					.filter((p) => !p.available)
					.map((p) => `${p.name} (API key not set)`)
					.join(', ')}. Set the key in the server's environment and restart it.
			</p>
		{/if}
	{/if}
</SettingsSection>
