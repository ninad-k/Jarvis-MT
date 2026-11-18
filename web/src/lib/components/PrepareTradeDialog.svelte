<script lang="ts">
	import { tick } from 'svelte';
	import { fmtPrice } from '#lib/format.ts';
	import { fmtRisk, MIN_RISK_PERCENT, prepareChoices, riskCeiling, setupLine, viewDigits } from '#lib/market.ts';
	import { digitsFor, status } from '#lib/state/hud.svelte.ts';
	import { market } from '#lib/state/market.svelte.ts';
	import { preview } from '#lib/state/preview.svelte.ts';

	let dlg = $state<HTMLDialogElement>();
	let volumeInput = $state<HTMLInputElement>();
	let riskInput = $state<HTMLInputElement>();
	const uid = $props.id();

	const v = $derived(market.prepareView);
	const setup = $derived(v?.setup ?? null);
	const digits = $derived(v ? viewDigits(v, digitsFor(v.broker_symbol)) : 2);
	const choices = $derived(setup && v ? prepareChoices(setup, v.price) : []);
	// The largest risk to offer: the server's limit when /api/status carries it, else 2.
	const maxRisk = $derived(riskCeiling(status.data?.limits));
	const sizeInput = () => (market.sizeMode === 'risk' ? riskInput : volumeInput);

	// Open and close the native modal with the store.
	$effect(() => {
		if (!dlg) return;
		if (market.prepareOpen && !dlg.open) {
			dlg.showModal();
			void tick().then(() => sizeInput()?.focus());
		} else if (!market.prepareOpen && dlg.open) {
			dlg.close();
		}
	});

	async function submit(e: Event) {
		e.preventDefault();
		const dto = await market.submitPrepare(maxRisk);
		if (!dto) {
			if (market.volumeError || market.riskError) sizeInput()?.focus();
			return;
		}
		// Close this dialog first, then hand the preview to the usual confirmation modal.
		market.closePrepare();
		dlg?.close();
		preview.open(dto);
	}

	function onCancelEvent(e: Event) {
		// Esc closes the dialog, but not while a request is in flight.
		if (market.preparing) e.preventDefault();
	}

	function onClose() {
		if (market.prepareOpen) market.closePrepare();
	}
</script>

<dialog
	bind:this={dlg}
	aria-labelledby="{uid}-title"
	oncancel={onCancelEvent}
	onclose={onClose}
	class="m-auto w-[min(30rem,calc(100vw-1.5rem))] max-w-none bg-transparent p-0 text-ink backdrop:bg-void/80 backdrop:backdrop-blur-sm max-sm:mt-auto max-sm:mb-0 max-sm:w-full"
>
	{#if v && setup}
		<form
			class="hud max-h-[calc(100dvh-1.5rem)] overflow-y-auto border-line-hi!"
			data-testid="prepare-dialog"
			novalidate
			onsubmit={submit}
		>
			<header class="border-b border-line px-4 py-3">
				<h2 id="{uid}-title" class="text-sm font-semibold tracking-wide">Prepare trade · {v.symbol}</h2>
			</header>

			<div class="space-y-4 px-4 py-4">
				<p class="text-sm font-medium" data-testid="prepare-setup-line">{setupLine(setup, digits)}</p>

				<fieldset>
					<legend class="hud-title mb-1.5">Size by</legend>
					<div class="grid grid-cols-2 gap-2">
						{#each [{ id: 'volume', label: 'Volume' }, { id: 'risk', label: 'Risk %' }] as m (m.id)}
							<label
								class="flex cursor-pointer items-center justify-center gap-2 border px-3 py-2 text-xs font-medium has-[:checked]:border-accent has-[:checked]:bg-accent/10 has-[:checked]:text-accent has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent {market.sizeMode === m.id ? 'border-accent' : 'border-line-hi text-mute'}"
							>
								<input
									type="radio"
									name="{uid}-size"
									value={m.id}
									bind:group={market.sizeMode}
									class="sr-only"
									data-testid="size-mode-{m.id}"
								/>
								{m.label}
							</label>
						{/each}
					</div>
				</fieldset>

				{#if market.sizeMode === 'volume'}
					<div>
						<label for="{uid}-volume" class="hud-title block">Volume (lots)</label>
						<input
							id="{uid}-volume"
							bind:this={volumeInput}
							bind:value={market.volume}
							class="input num mt-1"
							type="text"
							inputmode="decimal"
							autocomplete="off"
							spellcheck="false"
							maxlength="8"
							placeholder="Lots"
							required
							aria-required="true"
							aria-invalid={market.volumeError !== null}
							aria-describedby="{uid}-volume-help {uid}-volume-error"
							oninput={() => (market.volumeError = null)}
						/>
						<p id="{uid}-volume-help" class="mt-1 text-2xs text-mute">
							You choose the size; there is no default. Up to 2 decimals. The safety limits still apply.
						</p>
						<p id="{uid}-volume-error" class="mt-1 min-h-4 text-xs text-down" role="alert" data-testid="volume-error">{market.volumeError ?? ''}</p>
					</div>
				{:else}
					<div>
						<label for="{uid}-risk" class="hud-title block">Risk (% of equity)</label>
						<input
							id="{uid}-risk"
							bind:this={riskInput}
							bind:value={market.riskPercent}
							class="input num mt-1"
							type="text"
							inputmode="decimal"
							autocomplete="off"
							spellcheck="false"
							maxlength="6"
							placeholder="Percent, for example 1"
							required
							aria-required="true"
							aria-invalid={market.riskError !== null}
							aria-describedby="{uid}-risk-help {uid}-risk-error"
							oninput={() => (market.riskError = null)}
						/>
						<p id="{uid}-risk-help" class="mt-1 text-2xs text-mute">
							You choose the risk; there is no default. From {fmtRisk(MIN_RISK_PERCENT)} to {fmtRisk(maxRisk)}%. The lot size is
							worked out from the distance to the stop loss and shown in the next step.
						</p>
						<p id="{uid}-risk-error" class="mt-1 min-h-4 text-xs text-down" role="alert" data-testid="risk-error">{market.riskError ?? ''}</p>
					</div>
				{/if}

				<fieldset>
					<legend class="hud-title mb-1.5">Order type</legend>
					<div class="grid gap-2">
						{#each choices as c (c.type)}
							<label
								class="flex cursor-pointer items-start gap-3 border px-3 py-2 text-xs has-[:checked]:border-accent has-[:checked]:bg-accent/10 has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent {market.orderType === c.type ? 'border-accent' : 'border-line-hi'}"
							>
								<input
									type="radio"
									name="{uid}-type"
									value={c.type}
									bind:group={market.orderType}
									class="mt-0.5 accent-[var(--color-accent)]"
									data-testid="order-type-{c.type}"
								/>
								<span class="min-w-0">
									<span class="block text-sm font-medium">{c.label} · <span class="num">{fmtPrice(c.price, digits)}</span></span>
									<span class="block text-mute">
										{#if c.type === 'market'}
											Fills now {c.basis}.
										{:else}
											Waits for the price to come to you, {c.basis}.
										{/if}
									</span>
								</span>
							</label>
						{/each}
					</div>
				</fieldset>

				<p class="text-2xs text-mute">
					This only prepares a preview. Nothing is sent to the market until you confirm it in the next step.
				</p>

				{#if market.prepareError}
					<p class="border border-down/60 bg-down/10 px-3 py-2 text-xs text-down" role="alert" data-testid="prepare-error">
						<span aria-hidden="true">△ </span>{market.prepareError}
					</p>
				{/if}

				<div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
					<button type="button" class="btn h-10 px-5" disabled={market.preparing} onclick={() => dlg?.close()}>Cancel</button>
					<button type="submit" class="btn btn-primary h-10 px-5" disabled={market.preparing} data-testid="prepare-submit">
						{market.preparing ? 'Preparing…' : 'Prepare'}
					</button>
				</div>
			</div>
		</form>
	{/if}
</dialog>
