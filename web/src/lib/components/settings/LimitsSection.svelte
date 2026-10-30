<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type LimitsResponse, type Weekday } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import {
		LIMIT_FIELDS,
		WEEKDAYS,
		WEEKDAY_LABEL,
		describeLimit,
		formToOverrides,
		limitsBody,
		hasOverrides,
		limitErrorField,
		limitsToForm,
		validateLimitsForm,
		type LimitErrors,
		type LimitField,
		type LimitsForm
	} from '#lib/settings-rules.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import SettingsSection from './SettingsSection.svelte';

	let { authEnabled }: { authEnabled: boolean | null } = $props();

	const LABEL: Record<LimitField, string> = {
		max_lot: 'Max lot per order',
		max_positions: 'Max positions',
		max_daily_loss: 'Max daily loss',
		allowed_symbols: 'Allowed symbols',
		trading_hours: 'Trading hours'
	};

	type Load = 'loading' | 'ready' | 'unsupported' | 'error';
	let load = $state<Load>('loading');
	let loadError = $state<string | null>(null);
	let limits = $state<LimitsResponse | null>(null);
	let form = $state<LimitsForm | null>(null);
	let base = $state<string>('');
	let totp = $state('');
	let totpError = $state<string | null>(null);
	let errors = $state<LimitErrors>({});
	let saving = $state(false);
	let feedback = $state<Feedback | null>(null);

	/** The login is on, so the server wants an authenticator code for every change here. */
	const needsCode = $derived(authEnabled !== false);
	const dirty = $derived(form !== null && JSON.stringify($state.snapshot(form)) !== base);
	const tightened = $derived(limits?.overrides != null && hasOverrides(limits.overrides));

	async function fetchLimits(): Promise<void> {
		try {
			const res = await api.limits();
			limits = res;
			const f = limitsToForm(res.effective, res.config);
			form = f;
			base = JSON.stringify(f);
			errors = {};
			load = 'ready';
		} catch (e) {
			const err = toApiError(e);
			if (err.status === 404 || err.code === 'unknown') load = 'unsupported';
			else {
				load = 'error';
				loadError = describeError(err);
			}
		}
	}

	onMount(() => void fetchLimits());

	function toggleDay(d: Weekday, on: boolean) {
		if (!form) return;
		form.days = on ? WEEKDAYS.filter((x) => x === d || form!.days.includes(x)) : form.days.filter((x) => x !== d);
	}

	function toggleSymbol(s: string, on: boolean) {
		if (!form || !limits) return;
		form.allowed_symbols = limits.config.allowed_symbols.filter((x) => (x === s ? on : form!.allowed_symbols.includes(x)));
	}

	/** Show a server refusal next to the field it names, or at the top when it names none. */
	function showError(e: unknown): void {
		const err = toApiError(e);
		feedback = null;
		if (err.code === 'unauthorized') {
			totpError = err.message && err.message !== 'Sign-in required' ? err.message : 'Code not accepted';
			return;
		}
		const field = err.code === 'limit_not_stricter' ? limitErrorField(err.message) : null;
		if (field) {
			errors = { ...errors, [field]: err.message };
			feedback = { kind: 'error', text: 'The server refused a value that is looser than the config. See the field below.' };
		} else {
			feedback = { kind: 'error', text: describeError(err) };
		}
	}

	function codeOk(): boolean {
		if (!needsCode) return true;
		if (!/^\d{6}$/.test(totp)) {
			totpError = 'Enter the 6-digit code from your authenticator app';
			document.getElementById('limits-totp')?.focus();
			return false;
		}
		return true;
	}

	async function apply() {
		if (!form || !limits) return;
		totpError = null;
		feedback = null;
		const problems = validateLimitsForm(form, limits.config);
		errors = problems;
		if (Object.keys(problems).length > 0) {
			feedback = { kind: 'error', text: 'Some values are not allowed. They are marked below.' };
			return;
		}
		const { body, stuckHours } = limitsBody(form, limits.config, limits.overrides);
		if (stuckHours) {
			errors = { trading_hours: 'The config has no trading hours, so narrowed hours can only be removed with "Reset to config".' };
			feedback = { kind: 'error', text: 'Some values are not allowed. They are marked below.' };
			return;
		}
		if (!hasOverrides(formToOverrides(form, limits.config)) && !tightened) {
			feedback = {
				kind: 'error',
				text: 'Every value equals the config, so there is nothing to tighten. Lower a value first.'
			};
			return;
		}
		if (!codeOk()) return;
		saving = true;
		try {
			await api.setLimits({ ...body, ...(totp ? { totp } : {}) });
			totp = '';
			await fetchLimits();
			feedback = { kind: 'ok', text: 'Stricter limits are active now, for every persona and session.' };
		} catch (e) {
			showError(e);
		} finally {
			saving = false;
		}
	}

	async function reset() {
		totpError = null;
		feedback = null;
		if (!codeOk()) return;
		saving = true;
		try {
			await api.resetLimits(totp || undefined);
			totp = '';
			await fetchLimits();
			feedback = { kind: 'ok', text: 'Back to the limits in the config file.' };
		} catch (e) {
			showError(e);
		} finally {
			saving = false;
		}
	}

	function revert() {
		if (!base) return;
		form = JSON.parse(base) as LimitsForm;
		errors = {};
		feedback = null;
	}
</script>

<SettingsSection
	id="limits"
	title="Trading limits"
	{dirty}
	{saving}
	canSave={load === 'ready'}
	saveLabel="Apply stricter limits"
	{feedback}
	onsave={apply}
	onrevert={revert}
>
	{#snippet intro()}
		<span class="block border-l-2 border-warn bg-warn/10 px-3 py-2 text-ink" data-testid="limits-explainer">
			<strong>Limits can only be tightened here.</strong> You can lower a number, remove symbols or shorten the trading hours.
			To raise a limit or allow more, edit the <span class="num">[limits]</span> section of the server's config file and restart it.
			Changes apply to every persona and session at once; confirmations already on screen keep the limits they were checked against.
		</span>
	{/snippet}

	{#snippet extra()}
		<button type="button" class="btn btn-danger" disabled={saving || !tightened} onclick={reset} data-testid="limits-reset">
			Reset to config
		</button>
	{/snippet}

	{#if load === 'loading'}
		<div class="space-y-2" aria-busy="true" aria-label="Loading limits">
			{#each [0, 1, 2] as i (i)}<div class="skeleton h-9"></div>{/each}
		</div>
	{:else if load === 'unsupported'}
		<p class="text-xs text-mute" role="status">This server does not offer limits here yet. Edit the config file to change limits.</p>
	{:else if load === 'error'}
		<p class="flex items-center gap-3 text-xs text-down" role="alert">
			<span><span aria-hidden="true">△ </span>{loadError}</span>
			<button type="button" class="underline" onclick={() => void fetchLimits()}>Retry</button>
		</p>
	{:else if limits && form}
		<!-- Config versus what the gate enforces now -->
		<div>
			<h3 class="hud-title mb-1">In force now</h3>
			<div class="border border-line bg-panel/60 text-xs" data-testid="limits-table" role="table" aria-label="Limits: config and effective">
				<div class="grid grid-cols-[7.5rem_minmax(0,1fr)_minmax(0,1fr)] gap-x-3 border-b border-line px-3 py-1.5 text-2xs tracking-widest text-mute uppercase" role="row">
					<span role="columnheader">Limit</span><span role="columnheader">Config file</span><span role="columnheader">Effective</span>
				</div>
				{#each LIMIT_FIELDS as f (f)}
					{@const cfg = describeLimit(limits.config, f)}
					{@const eff = describeLimit(limits.effective, f)}
					<div class="grid grid-cols-[7.5rem_minmax(0,1fr)_minmax(0,1fr)] gap-x-3 border-b border-line/60 px-3 py-2 last:border-b-0" role="row" data-testid="limit-row-{f}">
						<span class="text-mute" role="rowheader">{LABEL[f]}</span>
						<span class="num break-words text-ink" role="cell">{cfg}</span>
						<span class="num break-words {cfg === eff ? 'text-ink' : 'text-accent'}" role="cell" data-tightened={cfg !== eff}>
							{eff}{#if cfg !== eff}<span class="sr-only"> (tightened)</span>{/if}
						</span>
					</div>
				{/each}
			</div>
		</div>

		<div class="grid gap-4 sm:grid-cols-3">
			<div>
				<label for="lim-lot" class="hud-title block">Max lot per order</label>
				<input
					id="lim-lot"
					class="input num mt-1"
					type="text"
					inputmode="decimal"
					autocomplete="off"
					bind:value={form.max_lot}
					aria-invalid={errors.max_lot !== undefined}
					aria-describedby="lim-lot-msg"
				/>
				<p id="lim-lot-msg" class="mt-1 text-xs {errors.max_lot ? 'text-down' : 'text-mute'}" role={errors.max_lot ? 'alert' : undefined}>
					{errors.max_lot ?? `Config: ${describeLimit(limits.config, 'max_lot')} or less`}
				</p>
			</div>
			<div>
				<label for="lim-pos" class="hud-title block">Max positions</label>
				<input
					id="lim-pos"
					class="input num mt-1"
					type="text"
					inputmode="numeric"
					autocomplete="off"
					bind:value={form.max_positions}
					aria-invalid={errors.max_positions !== undefined}
					aria-describedby="lim-pos-msg"
				/>
				<p id="lim-pos-msg" class="mt-1 text-xs {errors.max_positions ? 'text-down' : 'text-mute'}" role={errors.max_positions ? 'alert' : undefined}>
					{errors.max_positions ?? `Config: ${limits.config.max_positions} or fewer (positions plus pending orders)`}
				</p>
			</div>
			<div>
				<label for="lim-loss" class="hud-title block">Max daily loss</label>
				<input
					id="lim-loss"
					class="input num mt-1"
					type="text"
					inputmode="decimal"
					autocomplete="off"
					bind:value={form.max_daily_loss}
					aria-invalid={errors.max_daily_loss !== undefined}
					aria-describedby="lim-loss-msg"
				/>
				<p id="lim-loss-msg" class="mt-1 text-xs {errors.max_daily_loss ? 'text-down' : 'text-mute'}" role={errors.max_daily_loss ? 'alert' : undefined}>
					{errors.max_daily_loss ?? `Config: ${describeLimit(limits.config, 'max_daily_loss')} or less (account currency)`}
				</p>
			</div>
		</div>

		<fieldset aria-describedby="lim-sym-msg">
			<legend class="hud-title mb-1">Allowed symbols</legend>
			<div class="flex flex-wrap gap-2" data-testid="limit-symbols">
				{#each limits.config.allowed_symbols as s (s)}
					{@const on = form.allowed_symbols.includes(s)}
					<label
						class="flex cursor-pointer items-center gap-2 border px-2.5 py-1.5 text-xs {on ? 'border-accent bg-accent/10 text-ink' : 'border-line bg-panel/60 text-mute'}"
					>
						<input
							type="checkbox"
							class="size-4 accent-[var(--color-accent)]"
							checked={on}
							onchange={(e) => toggleSymbol(s, e.currentTarget.checked)}
							aria-label="Allow {s}"
						/>
						<span class="num">{s}</span>
					</label>
				{/each}
			</div>
			<p id="lim-sym-msg" class="mt-1 text-xs {errors.allowed_symbols ? 'text-down' : 'text-mute'}" role={errors.allowed_symbols ? 'alert' : undefined}>
				{errors.allowed_symbols ?? 'Untick a symbol to stop new orders in it. Symbols the config does not allow are not listed.'}
			</p>
		</fieldset>

		<fieldset aria-describedby="lim-hours-msg" class="space-y-3">
			<legend class="hud-title mb-1">Trading hours</legend>
			<label class="flex items-center gap-3 text-sm text-ink">
				<input type="checkbox" class="size-4 accent-[var(--color-accent)]" bind:checked={form.hours_enabled} data-testid="hours-enabled" />
				Narrow the hours for new orders
			</label>
			<p class="text-xs text-mute">
				Config: {describeLimit(limits.config, 'trading_hours')} (broker server time). Closing and cancelling are always allowed.
			</p>
			{#if form.hours_enabled}
				<div class="flex flex-wrap gap-2" role="group" aria-label="Days">
					{#each WEEKDAYS as d (d)}
						{@const on = form.days.includes(d)}
						{@const outside = limits.config.trading_hours !== null && !limits.config.trading_hours.days.includes(d)}
						<label
							class="flex cursor-pointer items-center gap-1.5 border px-2.5 py-1.5 text-xs {on ? 'border-accent bg-accent/10 text-ink' : 'border-line bg-panel/60 text-mute'} {outside ? 'opacity-50' : ''}"
						>
							<input
								type="checkbox"
								class="size-4 accent-[var(--color-accent)]"
								checked={on}
								disabled={outside}
								onchange={(e) => toggleDay(d, e.currentTarget.checked)}
							/>
							{WEEKDAY_LABEL[d]}
						</label>
					{/each}
				</div>
				<div class="flex flex-wrap items-end gap-3">
					<div>
						<label for="lim-start" class="hud-title block">From</label>
						<input id="lim-start" type="time" class="input num mt-1 w-32" bind:value={form.start} />
					</div>
					<div>
						<label for="lim-end" class="hud-title block">Until</label>
						<input id="lim-end" type="time" class="input num mt-1 w-32" bind:value={form.end} />
					</div>
				</div>
			{/if}
			<p id="lim-hours-msg" class="text-xs text-down" role="alert">{errors.trading_hours ?? ''}</p>
		</fieldset>

		<div class="max-w-xs">
			<label for="limits-totp" class="hud-title block">Authenticator code{needsCode ? '' : ' (optional)'}</label>
			<input
				id="limits-totp"
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
				aria-invalid={totpError !== null}
				aria-describedby="limits-totp-msg"
				data-testid="limits-totp"
			/>
			<p id="limits-totp-msg" class="mt-1 text-xs {totpError ? 'text-down' : 'text-mute'}" role={totpError ? 'alert' : undefined}>
				{totpError ?? (needsCode ? 'Needed to apply or reset limits. The 6-digit code from your authenticator app.' : 'Sign-in is off on this server, so no code is needed.')}
			</p>
		</div>
	{/if}
</SettingsSection>
