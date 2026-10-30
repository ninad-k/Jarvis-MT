<script lang="ts">
	import { tick } from 'svelte';
	import { preview } from '#lib/state/preview.svelte.ts';
	import { account } from '#lib/state/hud.svelte.ts';
	import { voice } from '#lib/state/voice.svelte.ts';
	import { fmtPrice, fmtSigned, fmtVolume, tone } from '#lib/format.ts';
	import CountdownRing from './CountdownRing.svelte';

	let dlg = $state<HTMLDialogElement>();
	let focusTarget = $state<HTMLElement>();
	let closeBtn = $state<HTMLButtonElement>();

	const p = $derived(preview.preview);
	const phase = $derived(preview.phase);
	const cd = $derived(preview.cd);
	const batch = $derived(p?.is_batch === true);
	const second = $derived(p?.stage === 'second');
	const expired = $derived(preview.expired);
	const ccy = $derived(account.data?.currency ?? '');
	const MAX_TICKETS = 6;

	const okCount = $derived(preview.results.filter((r) => r.ok).length);
	const summary = $derived.by(() => {
		const n = preview.results.length;
		if (n > 0 && okCount === n)
			return { text: n === 1 ? 'Executed' : `Executed: all ${n} succeeded`, cls: 't-up' };
		if (okCount === 0) return { text: 'Failed: nothing was executed', cls: 't-down' };
		return { text: `Partly executed: ${okCount} of ${n} succeeded`, cls: 'text-warn' };
	});

	// Open/close the native modal dialog with the store.
	$effect(() => {
		if (!dlg) return;
		if (p && !dlg.open) {
			dlg.showModal();
			void tick().then(() => focusTarget?.focus());
		} else if (!p && dlg.open) {
			dlg.close();
		}
	});

	// Move focus to where the next decision is: Close on results/errors, the dialog body on a new stage.
	$effect(() => {
		void p?.token;
		const ph = phase;
		void tick().then(() => {
			if (!dlg?.open) return;
			if (ph === 'results' || ph === 'error') closeBtn?.focus();
			else if (ph === 'review') focusTarget?.focus();
		});
	});

	function onCancelEvent(e: Event) {
		// Esc: cancel the confirmation (or close the result); never leave the token dangling.
		e.preventDefault();
		if (phase === 'executing') return;
		if (phase === 'review') void preview.cancel();
		else preview.dismiss();
	}

	function onClose() {
		// The browser closed the dialog on its own (a second Esc press does this).
		if (!preview.preview) return;
		if (phase === 'executing') dlg?.showModal();
		else if (phase === 'review') void preview.cancel();
		else preview.dismiss();
	}

	function onKeydown(e: KeyboardEvent) {
		// Enter must never confirm a trade. Only Cancel/Close keep their Enter key.
		if (e.key === 'Enter' && !(e.target as HTMLElement).closest('[data-enter-ok]')) e.preventDefault();
		if (e.key === 'Tab') trapFocus(e);
	}

	/** Keep Tab inside the dialog: wrap from the last control to the first and back. */
	function trapFocus(e: KeyboardEvent) {
		if (!dlg) return;
		const els = [...dlg.querySelectorAll<HTMLElement>('button:not(:disabled), input, select, textarea, a[href]')];
		if (els.length === 0) return e.preventDefault();
		const first = els[0];
		const last = els[els.length - 1];
		const active = document.activeElement;
		if (e.shiftKey && (active === first || active === focusTarget || active === dlg)) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && active === last) {
			e.preventDefault();
			first.focus();
		}
	}

	const label = $derived(
		batch ? (second ? 'Confirm again' : 'Confirm batch') : 'Confirm'
	);
</script>

<dialog
	bind:this={dlg}
	aria-modal="true"
	aria-labelledby="cm-title"
	aria-describedby="cm-desc"
	oncancel={onCancelEvent}
	onclose={onClose}
	onkeydown={onKeydown}
	class="m-auto w-[min(34rem,calc(100vw-1.5rem))] max-w-none bg-transparent p-0 text-ink backdrop:bg-void/80 backdrop:backdrop-blur-sm max-sm:mt-auto max-sm:mb-0 max-sm:w-full"
>
	{#if p}
		<div
			class="hud max-h-[calc(100dvh-1.5rem)] overflow-y-auto {batch ? 'border-down! shadow-[0_0_0_1px_var(--color-down)]' : 'border-line-hi!'}"
			data-testid="confirm-modal"
			data-stage={p.stage}
			data-phase={phase}
		>
			<!-- Header -->
			<header
				class="flex items-center justify-between gap-3 border-b px-4 py-3 {batch
					? 'border-down/60 bg-down/15'
					: 'border-line'}"
			>
				<h2 id="cm-title" class="flex items-center gap-2 text-sm font-semibold tracking-wide">
					{#if phase === 'results'}
						Trade result
					{:else if phase === 'error'}
						Not executed
					{:else if batch}
						<span class="pill t-down">Batch</span> Confirm batch close
					{:else}
						Confirm trade
					{/if}
				</h2>
				{#if batch && (phase === 'review' || phase === 'executing')}
					<span class="num text-2xs tracking-widest text-mute uppercase">Step {second ? 2 : 1} of 2</span>
				{/if}
			</header>

			<div class="space-y-4 px-4 py-4">
				<!-- Focus lands here when the dialog opens; Space then stays free for push-to-talk. -->
				<div bind:this={focusTarget} tabindex="-1" class="space-y-4 outline-none">
					<p id="cm-desc" data-testid="preview-description" class="text-base leading-snug font-medium break-words">{p.description}</p>

					{#if phase === 'review' || phase === 'executing'}
						<dl class="grid grid-cols-2 gap-x-4 gap-y-2 text-xs sm:grid-cols-[auto_1fr]">
							<dt class="hud-title">Symbols</dt>
							<dd class="flex flex-wrap gap-1.5">
								{#each p.symbols as s (s)}<span class="num border border-line-hi px-1.5">{s}</span>{/each}
							</dd>
							{#if p.tickets.length}
								<dt class="hud-title">Tickets</dt>
								<dd class="num break-words text-mute">
									{#each p.tickets.slice(0, MAX_TICKETS) as t, i (t)}{i ? ', ' : ''}#{t}{/each}
									{#if p.tickets.length > MAX_TICKETS}
										<span> +{p.tickets.length - MAX_TICKETS} more</span>
									{/if}
								</dd>
							{/if}
							<dt class="hud-title">Volume</dt>
							<dd class="num">
								{fmtVolume(p.total_volume)} lots{#if p.count > 1}
									<span class="text-mute">&nbsp;· {p.count} {p.tickets.length ? 'positions' : 'orders'}</span>{/if}
							</dd>
						</dl>

						<div>
							<h3 class="hud-title mb-1.5">Estimated P/L {ccy ? `(${ccy})` : ''}</h3>
							<div class="grid grid-cols-3 gap-px border border-line bg-line text-center">
								{#each [{ k: 'Now', v: p.est_pnl.current }, { k: 'At SL', v: p.est_pnl.at_sl }, { k: 'At TP', v: p.est_pnl.at_tp }] as c (c.k)}
									<div class="bg-panel px-2 py-2">
										<div class="text-2xs tracking-widest text-mute uppercase">{c.k}</div>
										<div class="num text-sm font-semibold {c.v === null ? 'text-faint' : `t-${tone(c.v)}`}">
											{c.v === null ? '—' : fmtSigned(c.v)}
										</div>
									</div>
								{/each}
							</div>
						</div>

						{#if p.warnings.length}
							<ul class="space-y-1 border border-warn/50 bg-warn/10 px-3 py-2 text-xs text-warn" aria-label="Warnings">
								{#each p.warnings as w (w)}
									<li><span aria-hidden="true">△ </span><span class="sr-only">Warning: </span>{w}</li>
								{/each}
							</ul>
						{/if}

						{#if preview.stageMessage && second}
							<p
								class="border border-down/70 bg-down/15 px-3 py-2 text-sm font-semibold text-down"
								data-testid="second-stage-message"
							>
								{preview.stageMessage}
							</p>
						{/if}

						{#if preview.notice}
							<p class="border border-warn/60 bg-warn/10 px-3 py-2 text-xs text-warn" role="alert">{preview.notice}</p>
						{/if}

						<!-- Countdown -->
						<div class="flex items-center gap-3" data-testid="countdown">
							<CountdownRing
								fraction={cd.fraction}
								seconds={cd.seconds}
								urgent={cd.urgent}
								expired={expired}
								paused={preview.awaitingReadback && !expired}
							/>
							<div class="min-w-0 text-xs">
								{#if expired}
									<p class="text-sm font-semibold t-down">Expired</p>
									<p class="text-mute">Nothing was sent. Ask again for a fresh preview.</p>
								{:else if preview.awaitingReadback}
									<p class="text-sm font-medium">Reading the preview aloud…</p>
									<p class="text-mute">The 15 s window starts when the reading ends.</p>
								{:else}
									<p class="text-sm font-medium {cd.urgent ? 't-down' : ''}">
										{cd.seconds} s left to {second ? 'confirm again' : 'confirm'}
									</p>
									<p class="text-mute">Executes only after you confirm.</p>
								{/if}
							</div>
						</div>
					{:else if phase === 'results'}
						<p class="text-sm font-semibold {summary.cls}" data-testid="result-summary">{summary.text}</p>
						<ul class="divide-y divide-line border border-line text-xs" aria-label="Per-ticket results">
							{#each preview.results as r (r.ticket)}
								<li class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5 px-3 py-2" data-ok={r.ok}>
									<span class="flex items-center gap-2">
										<span class={r.ok ? 't-up' : 't-down'} aria-hidden="true">{r.ok ? '✓' : '✕'}</span>
										<span class="num font-medium">#{r.ticket}</span>
										<span class="sr-only">{r.ok ? 'succeeded' : 'failed'}</span>
									</span>
									<span class="num text-mute">
										{r.retcode} · {r.retcode_description}{#if r.ok && r.price != null && r.volume != null}
											· {fmtVolume(r.volume)} @ {fmtPrice(r.price, r.price >= 100 ? 2 : 5)}{/if}
									</span>
									{#if !r.ok && r.error && r.error !== r.retcode_description}
										<span class="basis-full text-down">{r.error}</span>
									{/if}
								</li>
							{/each}
						</ul>
						<p class="text-2xs text-mute">Positions and account are refreshing.</p>
					{:else if phase === 'error' && preview.failure}
						<div class="border px-3 py-3 {preview.failure.code === 'outcome_unknown' ? 'border-warn bg-warn/10 text-warn' : 'border-down/60 bg-down/10 text-down'}" role="alert" data-testid="confirm-error" data-code={preview.failure.code}>
							{#if preview.failure.code === 'outcome_unknown'}
								<p class="text-sm font-semibold">Outcome unknown — check positions</p>
								<p class="mt-1 text-xs">
									The request may or may not have reached the market. Positions are being refreshed; do not repeat
									the trade until you have looked.
								</p>
							{:else}
								<p class="text-sm font-semibold">{preview.failure.message}</p>
							{/if}
							<p class="num mt-1 text-2xs opacity-70">{preview.failure.code}</p>
						</div>
					{/if}
				</div>

				<!-- Voice hint and live region -->
				{#if (phase === 'review') && voice.enabled}
					<p class="text-2xs text-mute">Hold <kbd class="num border border-line-hi px-1">Space</kbd> and say “confirm” or “cancel”. Enter does not confirm.</p>
				{/if}
				<p class="sr-only" role="status" aria-live="polite" data-testid="live">{preview.liveText}</p>

				<!-- Actions -->
				<div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
					{#if phase === 'review' || phase === 'executing'}
						<button
							type="button"
							data-enter-ok
							disabled={phase === 'executing'}
							class="h-10 border border-line-hi px-5 text-xs font-medium tracking-widest text-mute uppercase transition-colors hover:border-accent hover:text-ink disabled:cursor-not-allowed disabled:opacity-40"
							onclick={() => preview.cancel()}
						>
							Cancel
						</button>
						<button
							type="button"
							disabled={!preview.canConfirm}
							class="h-10 border px-5 text-xs font-semibold tracking-widest uppercase transition-colors disabled:cursor-not-allowed disabled:border-line disabled:bg-transparent disabled:text-faint {batch
								? 'border-down bg-down text-void hover:bg-down/85'
								: 'border-accent bg-accent/20 text-accent hover:bg-accent/30'}"
							onclick={() => preview.confirm()}
						>
							{phase === 'executing' ? 'Sending…' : label}
						</button>
					{:else}
						<button
							type="button"
							bind:this={closeBtn}
							data-enter-ok
							class="h-10 border border-accent bg-accent/15 px-6 text-xs font-medium tracking-widest text-accent uppercase transition-colors hover:bg-accent/25"
							onclick={() => preview.dismiss()}
						>
							Close
						</button>
					{/if}
				</div>
			</div>
		</div>
	{/if}
</dialog>
