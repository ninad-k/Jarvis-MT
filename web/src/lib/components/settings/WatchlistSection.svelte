<script lang="ts">
	import { describeError, toApiError } from '#lib/errors.ts';
	import { Draft } from '#lib/state/draft.svelte.ts';
	import { settings } from '#lib/state/settings.svelte.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import { MAX_WATCHLIST, addSymbol, moveItem, watchlistError } from '#lib/settings-rules.ts';
	import SettingsSection from './SettingsSection.svelte';

	const draft = new Draft<string[]>(() => settings.data!.watchlist);
	let entry = $state('');
	let entryError = $state<string | null>(null);
	let saving = $state(false);
	let feedback = $state<Feedback | null>(null);

	const problem = $derived(watchlistError(draft.value));
	let input = $state<HTMLInputElement>();

	function add() {
		const r = addSymbol(draft.value, entry);
		if ('error' in r) {
			entryError = r.error;
			input?.focus();
			return;
		}
		draft.value = r.list;
		entry = '';
		entryError = null;
		feedback = null;
		input?.focus();
	}

	function move(i: number, delta: number) {
		draft.value = moveItem(draft.value, i, delta);
		feedback = null;
	}

	function remove(i: number) {
		draft.value = draft.value.filter((_, k) => k !== i);
		feedback = null;
	}

	async function save() {
		saving = true;
		feedback = null;
		try {
			const next = await settings.update({ watchlist: $state.snapshot(draft.value) });
			draft.adopt(next.watchlist);
			feedback = { kind: 'ok', text: `Watchlist saved (${next.watchlist.length} symbols).` };
		} catch (e) {
			feedback = { kind: 'error', text: describeError(toApiError(e)) };
		} finally {
			saving = false;
		}
	}
</script>

<SettingsSection
	id="watchlist"
	title="Watchlist"
	dirty={draft.dirty}
	{saving}
	canSave={problem === null}
	{feedback}
	onsave={save}
	onrevert={() => draft.revert()}
>
	{#snippet intro()}
		The symbols shown on the main screen, in this order. You can use a broker symbol or one of your spoken names
		(see Symbol aliases). Up to {MAX_WATCHLIST}.
	{/snippet}

	<div>
		<div class="flex items-end gap-2">
			<div class="min-w-0 flex-1">
				<label for="wl-add" class="hud-title block">Add a symbol</label>
				<input
					id="wl-add"
					bind:this={input}
					bind:value={entry}
					class="input num mt-1"
					placeholder="e.g. US30 or yellow metal"
					autocomplete="off"
					autocapitalize="none"
					spellcheck="false"
					maxlength="64"
					aria-invalid={entryError !== null}
					aria-describedby="wl-add-error"
					disabled={draft.value.length >= MAX_WATCHLIST}
					onkeydown={(e) => {
						if (e.key === 'Enter') {
							e.preventDefault();
							add();
						}
					}}
					oninput={() => (entryError = null)}
				/>
			</div>
			<button type="button" class="btn" disabled={entry.trim() === '' || draft.value.length >= MAX_WATCHLIST} onclick={add}>Add</button>
		</div>
		<p id="wl-add-error" class="mt-1 min-h-4 text-xs text-down" role="alert">{entryError ?? ''}</p>
	</div>

	<div>
		<p class="hud-title mb-1 flex justify-between">
			<span>Your list</span>
			<span class="num" data-testid="wl-count">{draft.value.length} / {MAX_WATCHLIST}</span>
		</p>
		{#if draft.value.length === 0}
			<p class="border border-line bg-panel/60 p-3 text-xs text-mute">
				The watchlist is empty. Add a symbol above.
			</p>
		{:else}
			<ol class="divide-y divide-line border border-line bg-panel/60" data-testid="wl-list">
				{#each draft.value as symbol, i (i)}
					<li class="flex items-center gap-2 px-3 py-1.5" data-testid="wl-item">
						<span class="num w-6 text-right text-2xs text-mute" aria-hidden="true">{i + 1}</span>
						<span class="num min-w-0 flex-1 truncate text-sm text-ink">{symbol}</span>
						<button
							type="button"
							class="grid size-8 place-items-center border border-line text-mute hover:border-accent hover:text-ink disabled:opacity-30"
							aria-label="Move {symbol} up"
							disabled={i === 0}
							onclick={() => move(i, -1)}
						>
							<span aria-hidden="true">↑</span>
						</button>
						<button
							type="button"
							class="grid size-8 place-items-center border border-line text-mute hover:border-accent hover:text-ink disabled:opacity-30"
							aria-label="Move {symbol} down"
							disabled={i === draft.value.length - 1}
							onclick={() => move(i, 1)}
						>
							<span aria-hidden="true">↓</span>
						</button>
						<button
							type="button"
							class="grid size-8 place-items-center border border-line text-mute hover:border-down hover:text-down"
							aria-label="Remove {symbol}"
							onclick={() => remove(i)}
						>
							<span aria-hidden="true">✕</span>
						</button>
					</li>
				{/each}
			</ol>
		{/if}
		{#if problem}
			<p class="mt-1 text-xs text-down" role="alert"><span aria-hidden="true">△ </span>{problem}</p>
		{/if}
	</div>
</SettingsSection>
