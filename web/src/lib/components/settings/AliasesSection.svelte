<script lang="ts">
	import { describeError, toApiError } from '#lib/errors.ts';
	import { Draft } from '#lib/state/draft.svelte.ts';
	import { settings } from '#lib/state/settings.svelte.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import {
		MAX_ALIASES,
		aliasesToRows,
		rowsToAliases,
		validateAliasRows,
		type AliasRow
	} from '#lib/settings-rules.ts';
	import SettingsSection from './SettingsSection.svelte';

	// Rows (not a map) while editing, so a half-typed name is not lost and order is stable.
	const draft = new Draft<AliasRow[]>(() => aliasesToRows(settings.data!.aliases));
	let saving = $state(false);
	let feedback = $state<Feedback | null>(null);
	/** Show messages for blank rows too, after a failed Save. */
	let attempted = $state(false);

	const check = $derived(validateAliasRows(draft.value));
	const configAliases = $derived(aliasesToRows(settings.data?.config_aliases ?? {}));

	const rowMessage = (r: AliasRow, i: number): string | null => {
		const m = check.rows[i];
		if (m === null) return null;
		const touched = r.spoken.trim() !== '' || r.symbol.trim() !== '';
		return touched || attempted ? m : null;
	};

	function addRow() {
		draft.value = [...draft.value, { spoken: '', symbol: '' }];
		feedback = null;
		// Focus the new row's first field once it exists.
		queueMicrotask(() => {
			const all = document.querySelectorAll<HTMLInputElement>('[data-alias-spoken]');
			all[all.length - 1]?.focus();
		});
	}

	function removeRow(i: number) {
		draft.value = draft.value.filter((_, k) => k !== i);
		feedback = null;
	}

	async function save() {
		attempted = true;
		if (!check.ok) {
			feedback = { kind: 'error', text: check.general ?? 'Fix the highlighted rows first.' };
			return;
		}
		saving = true;
		feedback = null;
		try {
			const next = await settings.update({ aliases: rowsToAliases($state.snapshot(draft.value)) });
			draft.adopt(aliasesToRows(next.aliases));
			attempted = false;
			feedback = { kind: 'ok', text: `Aliases saved (${Object.keys(next.aliases).length}).` };
		} catch (e) {
			feedback = { kind: 'error', text: describeError(toApiError(e)) };
		} finally {
			saving = false;
		}
	}
</script>

<SettingsSection
	id="aliases"
	title="Symbol aliases"
	dirty={draft.dirty}
	{saving}
	{feedback}
	onsave={save}
	onrevert={() => {
		draft.revert();
		attempted = false;
	}}
>
	{#snippet intro()}
		Names you can say or type that mean a broker symbol, like “yellow metal” for XAUUSD. Yours win over the ones in the
		config file. Up to {MAX_ALIASES}.
	{/snippet}

	{#if draft.value.length === 0}
		<p class="border border-line bg-panel/60 p-3 text-xs text-mute">You have no aliases of your own yet.</p>
	{:else}
		<ul class="space-y-2" data-testid="alias-rows">
			{#each draft.value as row, i (i)}
				{@const msg = rowMessage(row, i)}
				<li data-testid="alias-row">
					<div class="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto] items-end gap-2">
						<div class="min-w-0">
							<label for="alias-spoken-{i}" class="hud-title block">{i === 0 ? 'Spoken name' : ''}<span class="sr-only">{i === 0 ? '' : 'Spoken name'} (row {i + 1})</span></label>
							<input
								id="alias-spoken-{i}"
								data-alias-spoken
								class="input mt-1"
								bind:value={row.spoken}
								placeholder="yellow metal"
								autocomplete="off"
								autocapitalize="none"
								spellcheck="false"
								maxlength="64"
								aria-invalid={msg !== null}
								aria-describedby={msg ? `alias-msg-${i}` : undefined}
							/>
						</div>
						<span class="pb-2.5 text-mute" aria-hidden="true">→</span>
						<div class="min-w-0">
							<label for="alias-symbol-{i}" class="hud-title block">{i === 0 ? 'Broker symbol' : ''}<span class="sr-only">{i === 0 ? '' : 'Broker symbol'} (row {i + 1})</span></label>
							<input
								id="alias-symbol-{i}"
								class="input num mt-1"
								bind:value={row.symbol}
								placeholder="XAUUSD"
								autocomplete="off"
								autocapitalize="characters"
								spellcheck="false"
								maxlength="32"
								aria-invalid={msg !== null}
								aria-describedby={msg ? `alias-msg-${i}` : undefined}
							/>
						</div>
						<button
							type="button"
							class="grid size-10 place-items-center border border-line text-mute hover:border-down hover:text-down"
							aria-label="Remove alias {row.spoken || `row ${i + 1}`}"
							onclick={() => removeRow(i)}
						>
							<span aria-hidden="true">✕</span>
						</button>
					</div>
					{#if msg}
						<p id="alias-msg-{i}" class="mt-1 text-xs text-down" role="alert"><span aria-hidden="true">△ </span>{msg}</p>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}

	<div class="flex items-center justify-between gap-3">
		<button type="button" class="btn" disabled={draft.value.length >= MAX_ALIASES} onclick={addRow}>Add alias</button>
		<span class="num text-2xs text-mute" data-testid="alias-count">{draft.value.length} / {MAX_ALIASES}</span>
	</div>
	{#if check.general}
		<p class="text-xs text-down" role="alert"><span aria-hidden="true">△ </span>{check.general}</p>
	{/if}

	{#if configAliases.length > 0}
		<div data-testid="config-aliases">
			<h3 class="hud-title">From the config file (read-only)</h3>
			<dl class="mt-1 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 border border-line bg-panel/60 p-3 text-xs">
				{#each configAliases as a (a.spoken)}
					<dt class="text-mute">{a.spoken}</dt>
					<dd class="num text-ink">{a.symbol}</dd>
				{/each}
			</dl>
			<p class="mt-1 text-xs text-mute">To change these, edit the config file. An alias of your own with the same name takes priority.</p>
		</div>
	{/if}
</SettingsSection>
