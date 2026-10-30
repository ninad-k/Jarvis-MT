<script lang="ts">
	import { tick } from 'svelte';
	import { killSwitch } from '#lib/state/killswitch.svelte.ts';
	import { status } from '#lib/state/hud.svelte.ts';
	import { auth } from '#lib/state/auth.svelte.ts';
	import { describeError, type ApiError } from '#lib/errors.ts';

	let open = $state(false);
	let error = $state<string | null>(null);
	let reason = $state('');
	let totp = $state('');
	let root = $state<HTMLDivElement>();
	let trigger = $state<HTMLButtonElement>();
	let firstField = $state<HTMLElement>();

	const engaged = $derived(killSwitch.engaged);
	const needsCode = $derived(status.data?.auth?.enabled === true);
	const codeOk = $derived(/^\d{6}$/.test(totp));

	async function toggle() {
		if (open) return close();
		error = null;
		reason = '';
		totp = '';
		open = true;
		await tick();
		firstField?.focus();
	}

	function close(refocus = true) {
		open = false;
		if (refocus) trigger?.focus();
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		error = null;
		try {
			if (engaged) await killSwitch.release(needsCode ? totp : undefined);
			else await killSwitch.engage(reason);
			close();
		} catch (err) {
			const ae = err as ApiError;
			// A rejected code and an ended session are both 401; the session check tells them apart.
			if (ae.code === 'unauthorized' && engaged) {
				await auth.refresh();
				error = 'That code was not accepted. Check your authenticator and try again.';
			} else {
				error = describeError(ae);
			}
			totp = '';
		}
	}

	$effect(() => {
		if (!open) return;
		const away = (e: PointerEvent) => {
			if (!root?.contains(e.target as Node)) close(false);
		};
		const esc = (e: KeyboardEvent) => {
			if (e.key === 'Escape') {
				e.stopPropagation();
				close();
			}
		};
		document.addEventListener('pointerdown', away, true);
		document.addEventListener('keydown', esc, true);
		return () => {
			document.removeEventListener('pointerdown', away, true);
			document.removeEventListener('keydown', esc, true);
		};
	});
</script>

<div bind:this={root} class="relative">
	<button
		bind:this={trigger}
		type="button"
		class="num flex h-7 items-center gap-2 border px-2.5 text-2xs font-semibold tracking-widest whitespace-nowrap uppercase transition-colors {engaged
			? 'border-down bg-down text-void hover:bg-down/85'
			: 'border-down/60 text-down hover:bg-down/10'}"
		aria-haspopup="dialog"
		aria-expanded={open}
		aria-controls="kill-popover"
		onclick={toggle}
		data-testid="kill-toggle"
	>
		<svg viewBox="0 0 24 24" class="size-3.5" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
			<path d="M12 3v9M6.3 6.8a8 8 0 1 0 11.4 0" stroke-linecap="round" />
		</svg>
		<span class="max-sm:sr-only">{engaged ? 'Trading disabled' : 'Kill switch'}</span>
		<span class="sm:hidden" aria-hidden="true">{engaged ? 'OFF' : 'KILL'}</span>
	</button>

	{#if open}
		<div
			id="kill-popover"
			role="dialog"
			aria-label={engaged ? 'Release kill switch' : 'Engage kill switch'}
			class="absolute top-full right-0 z-40 mt-2 w-72 max-w-[calc(100vw-2rem)] border border-line-hi bg-deck p-3 shadow-[0_8px_24px_rgb(0_0_0/0.6)]"
		>
		<form class="space-y-3" onsubmit={submit}>
			{#if engaged}
				<p class="text-xs text-ink">
					<span class="font-semibold">Release the kill switch?</span> Trading actions become available again.
				</p>
				{#if needsCode}
					<div>
						<label for="kill-totp" class="hud-title block">Authenticator code</label>
						<input
							id="kill-totp"
							bind:this={firstField}
							bind:value={totp}
							type="text"
							inputmode="numeric"
							autocomplete="one-time-code"
							pattern={'[0-9]{6}'}
							maxlength="6"
							placeholder="123456"
							class="num mt-1 block h-9 w-full border border-line-hi bg-void/70 px-2 text-base tracking-[0.3em] text-ink placeholder:text-faint focus-visible:border-accent focus-visible:outline-none"
						/>
					</div>
				{/if}
			{:else}
				<p class="text-xs text-ink">
					<span class="font-semibold">Engage the kill switch?</span> Every trading action is refused at once and open
					confirmations are revoked.
				</p>
				<div>
					<label for="kill-reason" class="hud-title block">Reason (optional)</label>
					<input
						id="kill-reason"
						bind:this={firstField}
						bind:value={reason}
						type="text"
						maxlength="80"
						placeholder="manual"
						autocomplete="off"
						class="mt-1 block h-9 w-full border border-line-hi bg-void/70 px-2 text-base text-ink placeholder:text-faint focus-visible:border-accent focus-visible:outline-none sm:text-sm"
					/>
				</div>
			{/if}

			{#if error}<p class="text-xs text-down" role="alert">{error}</p>{/if}

			<div class="flex justify-end gap-2">
				<button
					type="button"
					class="h-8 border border-line-hi px-3 text-2xs tracking-widest text-mute uppercase hover:border-accent hover:text-ink"
					onclick={() => close()}
				>
					Cancel
				</button>
				<button
					type="submit"
					disabled={killSwitch.busy || (engaged && needsCode && !codeOk)}
					class="h-8 border px-3 text-2xs font-semibold tracking-widest uppercase disabled:cursor-not-allowed disabled:opacity-40 {engaged
						? 'border-up text-up hover:bg-up/10'
						: 'border-down bg-down text-void hover:bg-down/85'}"
				>
					{killSwitch.busy ? 'Working…' : engaged ? 'Release' : 'Engage'}
				</button>
			</div>
		</form>
		</div>
	{/if}
</div>
