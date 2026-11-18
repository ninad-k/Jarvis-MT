<script lang="ts">
	import { tick } from 'svelte';
	import { alertReached, cleanNote, directionHint, fmtAlertPrice, inferDirection, spokenSymbol, validateAlertPrice } from '#lib/alerts.ts';
	import type { AlertDirection } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import { fmtPrice } from '#lib/format.ts';
	import { alerts } from '#lib/state/alerts.svelte.ts';
	import { watchlist } from '#lib/state/hud.svelte.ts';
	import { toasts } from '#lib/state/toasts.svelte.ts';
	import { symbolLabel } from '#lib/symbols.ts';

	const uid = $props.id();
	let dlg = $state<HTMLDialogElement>();
	let priceInput = $state<HTMLInputElement>();

	// The form starts empty every time: the price is the user's call, never a default.
	let price = $state('');
	let note = $state('');
	/** Set when the user picks a direction; otherwise it follows the price typed. */
	let chosen = $state<AlertDirection | null>(null);
	let priceError = $state<string | null>(null);
	let noteError = $state<string | null>(null);
	let formError = $state<string | null>(null);
	let saving = $state(false);
	/** The bell to give focus back to; its row can be re-drawn while the dialog is open. */
	let opener = '';

	const target = $derived(alerts.target);
	const tickNow = $derived(
		target ? watchlist.data?.prices.find((p) => symbolLabel(p).primary === target.symbol) : undefined
	);
	const digits = $derived(tickNow?.digits ?? null);
	const parsed = $derived(validateAlertPrice(price));
	const auto = $derived<AlertDirection | null>(parsed.ok && tickNow ? inferDirection(parsed.value, tickNow.bid) : null);
	const direction = $derived<AlertDirection>(chosen ?? auto ?? 'above');
	const immediate = $derived(
		parsed.ok && tickNow ? alertReached(direction, parsed.value, tickNow.bid, tickNow.ask) : false
	);

	$effect(() => {
		if (!dlg) return;
		if (target && !dlg.open) {
			price = '';
			note = '';
			chosen = null;
			priceError = noteError = formError = null;
			saving = false;
			opener = target.symbol;
			dlg.showModal();
			void tick().then(() => priceInput?.focus());
		} else if (!target && dlg.open) {
			dlg.close();
		}
	});

	async function submit(e: Event) {
		e.preventDefault();
		if (!target || saving) return;
		formError = null;
		const p = validateAlertPrice(price);
		const n = cleanNote(note);
		priceError = p.ok ? null : p.error;
		noteError = n.error;
		if (!p.ok) return priceInput?.focus();
		if (n.error) return;
		saving = true;
		try {
			const made = await alerts.create({
				symbol: target.symbol,
				price: p.value,
				direction,
				...(n.note ? { note: n.note } : {})
			});
			toasts.push(`Alert set: ${spokenSymbol(made.symbol)} ${made.direction} ${fmtAlertPrice(made.price, digits)}`);
			alerts.closeDialog();
		} catch (err) {
			formError = describeError(toApiError(err));
		} finally {
			saving = false;
		}
	}

	function onCancelEvent(e: Event) {
		if (saving) e.preventDefault();
	}

	function onClose() {
		if (alerts.target) alerts.closeDialog();
		// The row may have been re-drawn since the bell was pressed: find it again.
		const name = opener;
		void tick().then(() => document.querySelector<HTMLElement>(`[data-bell="${CSS.escape(name)}"]`)?.focus());
	}

	const OPTIONS: { id: AlertDirection; label: string }[] = [
		{ id: 'above', label: 'Above current price' },
		{ id: 'below', label: 'Below current price' }
	];
</script>

<dialog
	bind:this={dlg}
	aria-labelledby="{uid}-title"
	oncancel={onCancelEvent}
	onclose={onClose}
	class="m-auto w-[min(26rem,calc(100vw-1.5rem))] max-w-none bg-transparent p-0 text-ink backdrop:bg-void/80 backdrop:backdrop-blur-sm max-sm:mt-auto max-sm:mb-0 max-sm:w-full"
>
	{#if target}
		<form class="hud max-h-[calc(100dvh-1.5rem)] overflow-y-auto border-line-hi!" data-testid="alert-dialog" novalidate onsubmit={submit}>
			<header class="border-b border-line px-4 py-3">
				<h2 id="{uid}-title" class="text-sm font-semibold tracking-wide">Price alert · {target.symbol}</h2>
			</header>

			<div class="space-y-4 px-4 py-4">
				<p class="num text-xs text-mute" data-testid="alert-current">
					{#if tickNow}
						Now: <span class="sr-only">bid </span>{fmtPrice(tickNow.bid, tickNow.digits)}
						<span aria-hidden="true">/</span><span class="sr-only"> ask </span>{fmtPrice(tickNow.ask, tickNow.digits)}
					{:else}
						No live price right now.
					{/if}
				</p>

				<div>
					<label for="{uid}-price" class="hud-title block">Alert me when the price reaches</label>
					<input
						id="{uid}-price"
						bind:this={priceInput}
						bind:value={price}
						class="input num mt-1"
						type="text"
						inputmode="decimal"
						autocomplete="off"
						spellcheck="false"
						maxlength="14"
						placeholder="Price"
						required
						aria-required="true"
						aria-invalid={priceError !== null}
						aria-describedby="{uid}-price-error {uid}-direction-hint"
						oninput={() => (priceError = null)}
					/>
					<p id="{uid}-price-error" class="mt-1 min-h-4 text-xs text-down" role="alert" data-testid="alert-price-error">{priceError ?? ''}</p>
				</div>

				<fieldset>
					<legend class="hud-title mb-1.5">Direction</legend>
					<div class="grid grid-cols-2 gap-2">
						{#each OPTIONS as o (o.id)}
							<label
								class="flex cursor-pointer items-center gap-2 border px-3 py-2 text-xs has-[:checked]:border-accent has-[:checked]:bg-accent/10 has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent {direction === o.id ? 'border-accent' : 'border-line-hi'}"
							>
								<input
									type="radio"
									name="{uid}-direction"
									value={o.id}
									checked={direction === o.id}
									class="accent-[var(--color-accent)]"
									data-testid="alert-direction-{o.id}"
									onchange={() => (chosen = o.id)}
								/>
								<span>{o.label}</span>
							</label>
						{/each}
					</div>
					<p id="{uid}-direction-hint" class="mt-1.5 text-2xs text-mute" data-testid="alert-direction-hint">
						{directionHint(direction, chosen === null && auto !== null)}
						{#if auto === null && chosen === null}Enter a price and the direction is chosen from the current price.{/if}
					</p>
					{#if immediate}
						<p class="mt-1.5 border border-warn/60 bg-warn/10 px-2 py-1 text-xs text-warn" data-testid="alert-immediate">
							<span aria-hidden="true">△ </span>The price has already reached this level, so the alert will fire straight away.
						</p>
					{/if}
				</fieldset>

				<div>
					<label for="{uid}-note" class="hud-title block">Note <span class="font-normal normal-case text-mute">(optional)</span></label>
					<input
						id="{uid}-note"
						bind:value={note}
						class="input mt-1"
						type="text"
						autocomplete="off"
						maxlength="140"
						placeholder="Why this level matters"
						aria-invalid={noteError !== null}
						aria-describedby="{uid}-note-error"
						oninput={() => (noteError = null)}
					/>
					<p id="{uid}-note-error" class="mt-1 min-h-4 text-xs text-down" role="alert">{noteError ?? ''}</p>
				</div>

				<p class="text-2xs text-mute">It fires once, then moves to Recent. It only tells you; it never trades.</p>

				{#if formError}
					<p class="border border-down/60 bg-down/10 px-3 py-2 text-xs text-down" role="alert" data-testid="alert-form-error">
						<span aria-hidden="true">△ </span>{formError}
					</p>
				{/if}

				<div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
					<button type="button" class="btn h-10 px-5" disabled={saving} onclick={() => alerts.closeDialog()}>Cancel</button>
					<button type="submit" class="btn btn-primary h-10 px-5" disabled={saving} data-testid="alert-submit">
						{saving ? 'Setting…' : 'Set alert'}
					</button>
				</div>
			</div>
		</form>
	{/if}
</dialog>
