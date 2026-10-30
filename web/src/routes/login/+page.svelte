<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import { auth } from '#lib/state/auth.svelte.ts';
	import { resetPollers } from '#lib/state/hud.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { describeError, type ApiError } from '#lib/errors.ts';

	let username = $state('');
	let password = $state('');
	let totp = $state('');
	let error = $state<string | null>(null);
	let userInput = $state<HTMLInputElement>();

	const valid = $derived(username.trim().length > 0 && password.length > 0 && /^\d{6}$/.test(totp));

	onMount(() => {
		// The HUD's pollers have stopped by now; drop whatever the previous session left in them.
		resetPollers();
		persona.reset();
		userInput?.focus();
		// Already signed in (or no auth on the server)? Go straight to the HUD.
		api
			.me()
			.then(() => goto('/', { replace: true }))
			.catch(() => {});
	});

	function onTotp(e: Event & { currentTarget: HTMLInputElement }) {
		totp = e.currentTarget.value.replace(/\D/g, '').slice(0, 6);
		e.currentTarget.value = totp;
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!valid || auth.busy) return;
		error = null;
		try {
			await auth.login({ username: username.trim(), password, totp });
			password = '';
			await goto('/', { replace: true });
		} catch (err) {
			const ae = err as ApiError;
			error =
				ae.code === 'unauthorized'
					? 'Sign-in failed. Check your username, password and 6-digit code.'
					: describeError(ae);
			password = '';
			totp = '';
		}
	}
</script>

<svelte:head><title>Sign in · Arc MT</title></svelte:head>

<main class="grid min-h-dvh place-items-center overflow-y-auto p-4">
	<form class="hud w-full max-w-sm space-y-4 p-5" onsubmit={submit} aria-labelledby="login-title">
		<div class="flex items-center gap-3">
			<span class="grid size-8 place-items-center rounded-full border border-accent/70" aria-hidden="true">
				<span class="size-3 rounded-full bg-accent shadow-[0_0_10px_3px_color-mix(in_srgb,var(--color-accent)_70%,transparent)]"></span>
			</span>
			<div>
				<h1 id="login-title" class="font-mono text-sm font-semibold tracking-[0.28em]">
					ARC<span class="text-accent"> MT</span>
				</h1>
				<p class="text-xs text-mute">Sign in to continue</p>
			</div>
		</div>

		<div>
			<label for="username" class="hud-title block">Username</label>
			<input
				id="username"
				bind:this={userInput}
				bind:value={username}
				type="text"
				name="username"
				autocomplete="username"
				autocapitalize="none"
				spellcheck="false"
				required
				class="mt-1 block h-10 w-full border border-line-hi bg-void/70 px-3 text-base text-ink focus-visible:border-accent focus-visible:outline-none"
			/>
		</div>
		<div>
			<label for="password" class="hud-title block">Password</label>
			<input
				id="password"
				bind:value={password}
				type="password"
				name="password"
				autocomplete="current-password"
				required
				class="mt-1 block h-10 w-full border border-line-hi bg-void/70 px-3 text-base text-ink focus-visible:border-accent focus-visible:outline-none"
			/>
		</div>
		<div>
			<label for="totp" class="hud-title block">Authenticator code</label>
			<input
				id="totp"
				value={totp}
				oninput={onTotp}
				type="text"
				name="totp"
				inputmode="numeric"
				autocomplete="one-time-code"
				pattern={'[0-9]{6}'}
				maxlength="6"
				placeholder="123456"
				required
				aria-describedby="totp-help"
				class="num mt-1 block h-10 w-full border border-line-hi bg-void/70 px-3 text-base tracking-[0.4em] text-ink placeholder:text-faint focus-visible:border-accent focus-visible:outline-none"
			/>
			<p id="totp-help" class="mt-1 text-2xs text-mute">The 6-digit code from your authenticator app.</p>
		</div>

		{#if error}
			<p class="border border-down/60 bg-down/10 px-3 py-2 text-xs text-down" role="alert">{error}</p>
		{/if}

		<button
			type="submit"
			disabled={!valid || auth.busy}
			class="h-10 w-full border border-accent bg-accent/15 text-xs font-semibold tracking-widest text-accent uppercase transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:border-line disabled:bg-transparent disabled:text-faint"
		>
			{auth.busy ? 'Signing in…' : 'Sign in'}
		</button>
	</form>
</main>
