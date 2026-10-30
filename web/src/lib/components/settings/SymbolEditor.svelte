<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { api, type CatalogSymbol, type SymbolCandidate } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import { symbolMap } from '#lib/state/symbols.svelte.ts';
	import { filterCandidates, pluralize, rankCandidates } from '#lib/symbols.ts';

	let {
		canonical,
		current,
		overridden,
		candidates,
		catalogSize,
		needsCode,
		onsaved,
		oncancel
	}: {
		canonical: string;
		/** The broker symbol in force now (empty for an unmapped instrument). */
		current: string;
		/** A user override exists, so "Reset to automatic" is offered. */
		overridden: boolean;
		candidates: SymbolCandidate[];
		catalogSize: number | null;
		/** Login is on, so the server wants an authenticator code. */
		needsCode: boolean;
		onsaved: (text: string) => void;
		oncancel: () => void;
	} = $props();

	const DEBOUNCE_MS = 250;
	const uid = $props.id();
	const listId = `${uid}-list`;
	const optionId = (i: number) => `${uid}-opt-${i}`;

	interface Option {
		name: string;
		description: string;
		detail: string;
		group: 'Suggested' | 'Catalog';
	}

	// svelte-ignore state_referenced_locally
	let query = $state(current);
	let open = $state(false);
	let active = $state(-1);
	let results = $state<CatalogSymbol[]>([]);
	let searching = $state(false);
	let searchError = $state<string | null>(null);
	let totp = $state('');
	let totpError = $state<string | null>(null);
	let error = $state<string | null>(null);
	let saving = $state<'save' | 'reset' | null>(null);

	let input = $state<HTMLInputElement>();
	let wrapper = $state<HTMLElement>();
	/** The user has typed, so the suggestions are narrowed to what they typed. */
	let touched = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;
	let ctrl: AbortController | null = null;
	/** Picking an option writes its name into the box; that must not start another search. */
	let skipSearch = true;

	const ranked = $derived(rankCandidates(candidates, current));
	const suggestions = $derived(filterCandidates(ranked, touched ? query : ''));
	const options = $derived.by<Option[]>(() => {
		const out: Option[] = suggestions.map((c) => ({
			name: c.broker_symbol,
			description: c.description,
			detail: `score ${c.score}${c.reasons.length ? ` · ${c.reasons.join(', ')}` : ''}`,
			group: 'Suggested'
		}));
		const seen = new Set(out.map((o) => o.name));
		for (const s of results) {
			if (seen.has(s.name)) continue;
			out.push({ name: s.name, description: s.description, detail: catalogDetail(s), group: 'Catalog' });
		}
		return out;
	});

	function catalogDetail(s: CatalogSymbol): string {
		const mode = typeof s.trade_mode === 'string' && /^full$/i.test(s.trade_mode) ? null : String(s.trade_mode);
		return [s.path, mode ? `trading: ${mode}` : null, s.visible ? null : 'hidden in Market Watch'].filter(Boolean).join(' · ');
	}

	// Debounced catalog search. An empty box shows only the suggestions.
	$effect(() => {
		const q = query.trim();
		clearTimeout(timer);
		ctrl?.abort();
		if (skipSearch) {
			skipSearch = false;
			return;
		}
		if (!q) {
			results = [];
			searching = false;
			searchError = null;
			return;
		}
		searching = true;
		timer = setTimeout(async () => {
			const mine = new AbortController();
			ctrl = mine;
			try {
				const res = await api.symbolCatalog(q, 30, mine.signal);
				if (mine.signal.aborted) return;
				results = res.symbols;
				searchError = null;
			} catch (e) {
				const err = toApiError(e);
				if (err.code === 'aborted') return;
				results = [];
				searchError = describeError(err);
			}
			searching = false;
		}, DEBOUNCE_MS);
	});

	// Focus moves into the editor when it opens, so a keyboard user can type straight away.
	onMount(() => input?.focus());

	onDestroy(() => {
		clearTimeout(timer);
		ctrl?.abort();
	});

	// Keep the highlighted option valid while the list changes.
	$effect(() => {
		if (active >= options.length) active = options.length - 1;
	});
	// Scroll the highlighted option into view for keyboard users.
	$effect(() => {
		if (open && active >= 0) document.getElementById(optionId(active))?.scrollIntoView({ block: 'nearest' });
	});

	function pick(i: number) {
		const o = options[i];
		if (!o) return;
		skipSearch = true;
		query = o.name;
		open = false;
		active = -1;
		error = null;
		input?.focus();
	}

	function onKeydown(e: KeyboardEvent) {
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				if (!open) open = true;
				else if (options.length) active = (active + 1) % options.length;
				break;
			case 'ArrowUp':
				e.preventDefault();
				if (!open) open = true;
				else if (options.length) active = active <= 0 ? options.length - 1 : active - 1;
				break;
			case 'Enter':
				e.preventDefault();
				if (open && active >= 0) pick(active);
				else void save();
				break;
			case 'Escape':
				// First Esc closes the list, a second one closes the editor.
				if (open) {
					e.preventDefault();
					e.stopPropagation();
					open = false;
					active = -1;
				}
				break;
			case 'Tab':
				open = false;
				break;
		}
	}

	function codeOk(): boolean {
		if (!needsCode) return true;
		if (!/^\d{6}$/.test(totp)) {
			totpError = 'Enter the 6-digit code from your authenticator app';
			document.getElementById(`${uid}-totp`)?.focus();
			return false;
		}
		return true;
	}

	async function send(brokerSymbol: string | null, kind: 'save' | 'reset') {
		error = null;
		totpError = null;
		open = false;
		if (kind === 'save' && !brokerSymbol) {
			error = 'Type or pick the broker symbol first.';
			input?.focus();
			return;
		}
		if (!codeOk()) return;
		saving = kind;
		try {
			await api.setSymbolMap({ canonical, broker_symbol: brokerSymbol, ...(totp ? { totp } : {}) });
			totp = '';
			await symbolMap.refresh();
			onsaved(
				brokerSymbol === null
					? `${canonical} is back to automatic mapping.`
					: `${canonical} now maps to ${brokerSymbol}.`
			);
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'unauthorized') {
				totpError = err.message && err.message !== 'Sign-in required' ? err.message : 'Code not accepted';
			} else if (err.code === 'unknown_broker_symbol') {
				error = err.message || `${brokerSymbol} is not in this broker's catalog.`;
				input?.focus();
			} else {
				error = describeError(err);
			}
		} finally {
			saving = null;
		}
	}

	const save = () => send(query.trim(), 'save');
	const reset = () => send(null, 'reset');
</script>

<!-- Esc closes the editor as a shortcut; every control in it stays reachable with Tab. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
	class="mt-3 space-y-3 border border-line-hi bg-raise/40 p-3"
	role="group"
	bind:this={wrapper}
	aria-label="Edit mapping for {canonical}"
	data-testid="symbol-editor"
	data-canonical={canonical}
	onkeydown={(e) => {
		if (e.key === 'Escape') oncancel();
	}}
>
	<div>
		<label for="{uid}-input" class="hud-title block">Broker symbol for {canonical}</label>
		<input
			bind:this={input}
			id="{uid}-input"
			class="input num mt-1"
			type="text"
			role="combobox"
			autocomplete="off"
			autocapitalize="off"
			spellcheck="false"
			placeholder="Search the broker's symbols"
			aria-autocomplete="list"
			aria-expanded={open}
			aria-controls={listId}
			aria-activedescendant={open && active >= 0 ? optionId(active) : undefined}
			aria-invalid={error !== null}
			aria-describedby="{uid}-hint {uid}-error"
			bind:value={query}
			onfocus={() => (open = true)}
			oninput={() => {
				touched = true;
				open = true;
				active = -1;
				error = null;
			}}
			onkeydown={onKeydown}
			onblur={(e) => {
				// Moving to a button in this editor keeps the list until that button is clicked: collapsing it
				// on mouse-down would shift the button away from the pointer and swallow the click.
				const to = e.relatedTarget;
				if (!(to instanceof Node && wrapper?.contains(to))) open = false;
			}}
			data-testid="symbol-combobox"
		/>
		<p id="{uid}-hint" class="mt-1 text-xs text-mute">
			{catalogSize
				? `Searches ${pluralize(catalogSize, 'symbol')} on this broker by name or description.`
				: "Searches this broker's symbol list by name or description."}
		</p>

		<ul
			id={listId}
			role="listbox"
			aria-label="Matching broker symbols for {canonical}"
			class="mt-2 max-h-60 overflow-y-auto border border-line bg-deck/90 {open ? '' : 'hidden'}"
			data-testid="symbol-options"
		>
			{#each options as o, i (`${o.group}:${o.name}`)}
				{#if i === 0 || options[i - 1].group !== o.group}
					<li role="presentation" class="hud-title border-b border-line/60 bg-panel/60 px-3 py-1">
						{o.group === 'Suggested' ? 'Suggested for this instrument' : 'Broker catalog'}
					</li>
				{/if}
				<!-- The input owns the keyboard (combobox pattern); the click is for pointer users. -->
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<li
					id={optionId(i)}
					role="option"
					aria-selected={i === active}
					class="cursor-pointer border-b border-line/40 px-3 py-2 last:border-b-0 {i === active ? 'bg-accent/15' : 'hover:bg-raise/60'}"
					onmousedown={(e) => e.preventDefault()}
					onclick={() => pick(i)}
					data-testid="symbol-option"
				>
					<span class="num block text-sm break-all text-ink">{o.name}</span>
					{#if o.description}<span class="block text-xs break-words text-mute">{o.description}</span>{/if}
					{#if o.detail}<span class="block text-2xs break-words text-faint">{o.detail}</span>{/if}
				</li>
			{/each}
			{#if options.length === 0}
				<li role="presentation" class="px-3 py-2 text-xs text-mute">
					{#if searching}
						Searching…
					{:else if searchError}
						<span class="text-down">{searchError}</span>
					{:else if query.trim()}
						No symbols match "{query.trim()}".
					{:else}
						Type to search. No suggestions for this instrument.
					{/if}
				</li>
			{/if}
		</ul>
		<p class="sr-only" role="status" aria-live="polite">
			{#if open}{searching ? 'Searching' : pluralize(options.length, 'match', 'matches')}{/if}
		</p>
		<p id="{uid}-error" class="mt-1 min-h-4 text-xs text-down" role={error ? 'alert' : undefined} data-testid="symbol-error">
			{#if error}<span aria-hidden="true">△ </span>{error}{/if}
		</p>
	</div>

	{#if needsCode}
		<div class="max-w-xs">
			<label for="{uid}-totp" class="hud-title block">Authenticator code</label>
			<input
				id="{uid}-totp"
				class="input num mt-1 tracking-[0.4em]"
				type="text"
				inputmode="numeric"
				autocomplete="one-time-code"
				maxlength="6"
				placeholder="123456"
				bind:value={totp}
				oninput={(e) => {
					totp = e.currentTarget.value.replace(/\D/g, '').slice(0, 6);
					e.currentTarget.value = totp;
					totpError = null;
				}}
				onkeydown={(e) => {
					if (e.key === 'Enter') {
						e.preventDefault();
						void save();
					}
				}}
				onfocus={() => (open = false)}
				aria-invalid={totpError !== null}
				aria-describedby="{uid}-totp-msg"
				data-testid="symbol-totp"
			/>
			<p id="{uid}-totp-msg" class="mt-1 text-xs {totpError ? 'text-down' : 'text-mute'}" role={totpError ? 'alert' : undefined}>
				{totpError ?? 'A wrong mapping could send an order to the wrong instrument, so a code is needed.'}
			</p>
		</div>
	{/if}

	<div class="flex flex-wrap items-center gap-2">
		<button type="button" class="btn btn-primary" disabled={saving !== null} onclick={save} data-testid="symbol-save">
			{saving === 'save' ? 'Saving…' : 'Save mapping'}
		</button>
		{#if overridden}
			<button type="button" class="btn btn-danger" disabled={saving !== null} onclick={reset} data-testid="symbol-reset">
				{saving === 'reset' ? 'Resetting…' : 'Reset to automatic'}
			</button>
		{/if}
		<button type="button" class="btn" disabled={saving !== null} onclick={oncancel}>Cancel</button>
	</div>
</div>
