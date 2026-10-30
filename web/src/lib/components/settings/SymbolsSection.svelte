<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import { symbolMap } from '#lib/state/symbols.svelte.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import {
		affixText,
		aliasLine,
		attentionCount,
		brokerLine,
		exportFileName,
		fmtDetected,
		groupMappings,
		pluralize
	} from '#lib/symbols.ts';
	import SettingsSection from './SettingsSection.svelte';
	import SourceBadge from './SourceBadge.svelte';
	import SymbolEditor from './SymbolEditor.svelte';

	let { authEnabled }: { authEnabled: boolean | null } = $props();

	let editing = $state<string | null>(null);
	let feedback = $state<Feedback | null>(null);
	let exporting = $state(false);

	const data = $derived(symbolMap.data);
	const broker = $derived(data?.broker ?? null);
	const groups = $derived(groupMappings(data?.mappings ?? []));
	const attention = $derived(data ? attentionCount(data.mappings, data.unmapped) : 0);
	/** The login is on, so the server wants an authenticator code for a mapping change. */
	const needsCode = $derived(authEnabled !== false);

	onMount(() => void symbolMap.refresh(true));

	async function redetect() {
		feedback = null;
		editing = null;
		try {
			const next = await symbolMap.detect();
			feedback = {
				kind: 'ok',
				text: next.broker
					? `Detection finished for ${next.broker.company}: ${pluralize(next.mappings.length, 'instrument')} mapped, ${next.unmapped.length} unmapped.`
					: 'Detection finished, but the MT5 bridge has not reported a broker yet.'
			};
		} catch (e) {
			feedback = { kind: 'error', text: describeError(toApiError(e)) };
		}
	}

	async function exportJson() {
		feedback = null;
		exporting = true;
		try {
			const blob = await api.exportSymbols();
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = exportFileName(broker);
			document.body.append(a);
			a.click();
			a.remove();
			setTimeout(() => URL.revokeObjectURL(url), 2000);
			feedback = { kind: 'ok', text: `Exported the map as ${a.download}.` };
		} catch (e) {
			feedback = { kind: 'error', text: describeError(toApiError(e)) };
		} finally {
			exporting = false;
		}
	}

	/** Close the editor and put focus back on the button that opened it. */
	async function closeEditor() {
		const key = editing;
		editing = null;
		await tick();
		if (key) document.querySelector<HTMLElement>(`[data-edit-key="${CSS.escape(key)}"]`)?.focus();
	}

	function saved(text: string) {
		void closeEditor();
		feedback = { kind: 'ok', text };
	}

	const rowBtn = 'btn h-8 px-2.5';
</script>

<SettingsSection id="symbols" title="Symbol mapping" hideActions {feedback}>
	{#snippet intro()}
		Arc MT uses one canonical name for each instrument (<span class="num">US100</span>, <span class="num">XAUUSD</span>) and maps it to
		your broker's own symbol. Limits, the watchlist and chat keep working when the broker changes. Your own choice beats the curated
		list, which beats automatic detection.
	{/snippet}

	{#if symbolMap.load === 'loading' || symbolMap.load === 'idle'}
		<div class="space-y-2" aria-busy="true" aria-label="Loading symbol mapping">
			{#each [0, 1, 2, 3] as i (i)}<div class="skeleton h-9"></div>{/each}
		</div>
	{:else if symbolMap.load === 'unsupported'}
		<p class="text-xs text-mute" role="status">This server does not map broker symbols yet. Update the server to use this section.</p>
	{:else if symbolMap.load === 'error' && !data}
		<p class="flex items-center gap-3 text-xs text-down" role="alert">
			<span><span aria-hidden="true">△ </span>{symbolMap.error}</span>
			<button type="button" class="underline" onclick={() => void symbolMap.refresh(true)}>Retry</button>
		</p>
	{:else if data}
		<!-- Broker, detection and the two buttons -->
		<div class="border border-line bg-panel/60 p-3" data-testid="symbols-header">
			<div class="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
				<div class="min-w-0 flex-[1_1_14rem]">
					<h3 class="hud-title">Detected broker</h3>
					{#if broker}
						<p class="mt-1 text-sm font-medium break-words text-ink" data-testid="symbols-broker">{brokerLine(broker)}</p>
					{:else}
						<p class="mt-1 text-sm text-mute" data-testid="symbols-broker">Not detected yet</p>
						<p class="mt-0.5 text-xs text-mute">The MT5 bridge has not been reachable. Run detection once it is connected.</p>
					{/if}
				</div>
				<div class="flex flex-wrap items-center gap-2">
					<button
						type="button"
						class="btn"
						disabled={symbolMap.detecting}
						onclick={redetect}
						aria-describedby="symbols-detect-status"
						data-testid="symbols-redetect"
					>
						{symbolMap.detecting ? 'Detecting…' : 'Re-detect'}
					</button>
					<button type="button" class="btn" disabled={exporting || !broker} onclick={exportJson} data-testid="symbols-export">
						{exporting ? 'Exporting…' : 'Export JSON'}
					</button>
				</div>
			</div>

			{#if broker}
				<dl class="mt-3 grid grid-cols-2 gap-x-4 gap-y-2 text-xs sm:grid-cols-4">
					<div>
						<dt class="text-2xs tracking-widest text-faint uppercase">Detected</dt>
						<dd class="num text-ink" data-testid="symbols-detected-at">{fmtDetected(broker.detected_at)}</dd>
					</div>
					<div>
						<dt class="text-2xs tracking-widest text-faint uppercase">Catalog</dt>
						<dd class="num text-ink" data-testid="symbols-catalog">{pluralize(broker.catalog_size, 'symbol')}</dd>
					</div>
					<div>
						<dt class="text-2xs tracking-widest text-faint uppercase">Suffix</dt>
						<dd class="num text-ink" data-testid="symbols-suffix">{affixText(broker.detected_suffix)}</dd>
					</div>
					<div>
						<dt class="text-2xs tracking-widest text-faint uppercase">Prefix</dt>
						<dd class="num text-ink" data-testid="symbols-prefix">{affixText(broker.detected_prefix)}</dd>
					</div>
				</dl>
			{/if}

			<div id="symbols-detect-status" class="mt-2 min-h-4 text-xs" role="status" aria-live="polite">
				{#if symbolMap.detecting}
					<span class="text-accent">Reading the broker's symbol list and matching it. This can take a few seconds.</span>
					<span class="mt-1.5 block h-0.5 w-full overflow-hidden bg-line" aria-hidden="true">
						<span class="block h-full w-1/3 animate-[detect-slide_1.1s_ease-in-out_infinite] bg-accent"></span>
					</span>
				{:else if attention > 0}
					<span class="text-warn" data-testid="symbols-attention">
						<span aria-hidden="true" class="mr-1">△</span>{pluralize(attention, 'instrument')} need{attention === 1 ? 's' : ''} attention.
					</span>
				{/if}
			</div>
		</div>

		<!-- The map, by instrument class -->
		{#each groups as g (g.id)}
			<div data-testid="symbol-group" data-group={g.id}>
				<h3 class="hud-title mb-1 flex items-center justify-between">
					<span>{g.label}</span><span class="num text-faint">{g.rows.length}</span>
				</h3>
				<ul class="border border-line bg-panel/60" aria-label="{g.label} symbol mappings">
					{#each g.rows as m (m.canonical)}
						<li class="border-t border-line/60 px-3 py-2.5 first:border-t-0" data-testid="symbol-row" data-canonical={m.canonical} data-source={m.source} data-available={m.available}>
							<div class="flex flex-wrap items-start justify-between gap-x-3 gap-y-2">
								<p class="min-w-0 flex-[1_1_11rem] text-xs break-words" title={aliasLine(m)} data-testid="symbol-name">
									<span class="num text-sm font-medium text-ink">{m.canonical}</span>
									{#if m.aliases.length}<span class="text-mute"> · {m.aliases.join(', ')}</span>{/if}
								</p>
								<div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1.5">
									<span class="num text-sm break-all {m.available ? 'text-ink' : 'text-warn'}" data-testid="symbol-broker">{m.broker_symbol}</span>
									<SourceBadge source={m.source} id="src-{m.canonical}" />
									{#if !m.available}
										<span class="text-xs text-warn" data-testid="symbol-unavailable">
											<span aria-hidden="true" class="mr-1">△</span>Not in this broker's catalog
										</span>
									{/if}
									<button
										type="button"
										class={rowBtn}
										aria-expanded={editing === m.canonical}
										aria-label="Edit mapping for {m.canonical}"
										onclick={() => (editing = editing === m.canonical ? null : m.canonical)}
										data-testid="symbol-edit"
										data-edit-key={m.canonical}
									>
										Edit
									</button>
								</div>
							</div>
							{#if editing === m.canonical}
								<SymbolEditor
									canonical={m.canonical}
									current={m.broker_symbol}
									overridden={m.source === 'user'}
									candidates={m.candidates}
									catalogSize={broker?.catalog_size ?? null}
									{needsCode}
									onsaved={saved}
									oncancel={() => void closeEditor()}
								/>
							{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/each}

		{#if groups.length === 0}
			<p class="text-xs text-mute" role="status">No instruments are configured on this server.</p>
		{/if}

		<!-- Instruments this broker does not seem to offer -->
		{#if data.unmapped.length > 0}
			<div data-testid="symbols-unmapped">
				<h3 class="hud-title mb-1 flex items-center justify-between">
					<span>Unmapped on this broker</span><span class="num text-faint">{data.unmapped.length}</span>
				</h3>
				<p class="mb-1.5 text-xs text-mute">
					No matching symbol was found. Orders in these instruments will be refused until you pick one.
				</p>
				<ul class="border border-line bg-panel/60" aria-label="Unmapped instruments">
					{#each data.unmapped as name (name)}
						<li class="border-t border-line/60 px-3 py-2.5 first:border-t-0" data-testid="unmapped-row" data-canonical={name}>
							<div class="flex flex-wrap items-center justify-between gap-2">
								<span class="num text-sm font-medium text-ink">{name}</span>
								<button
									type="button"
									class={rowBtn}
									aria-expanded={editing === `unmapped:${name}`}
									aria-label="Map {name} to a broker symbol"
									onclick={() => (editing = editing === `unmapped:${name}` ? null : `unmapped:${name}`)}
									data-testid="unmapped-edit"
									data-edit-key="unmapped:{name}"
								>
									Map…
								</button>
							</div>
							{#if editing === `unmapped:${name}`}
								<SymbolEditor
									canonical={name}
									current=""
									overridden={false}
									candidates={[]}
									catalogSize={broker?.catalog_size ?? null}
									{needsCode}
									onsaved={saved}
									oncancel={() => void closeEditor()}
								/>
							{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/if}
	{/if}
</SettingsSection>
