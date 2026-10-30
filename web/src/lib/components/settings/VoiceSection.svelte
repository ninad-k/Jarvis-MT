<script lang="ts">
	import type { InputMode, VoiceLanguage, VoiceSettings } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import { Draft } from '#lib/state/draft.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { settings } from '#lib/state/settings.svelte.ts';
	import type { Feedback } from '#lib/state/settings-context.ts';
	import SettingsSection from './SettingsSection.svelte';

	let { ttsAvailable, sttAvailable }: { ttsAvailable: boolean; sttAvailable: boolean } = $props();

	const draft = new Draft<VoiceSettings>(() => settings.data!.voice);
	let saving = $state(false);
	let feedback = $state<Feedback | null>(null);

	const MODES: { id: InputMode; label: string; help: string }[] = [
		{ id: 'hold', label: 'Hold to talk', help: 'Hold the mic button or the Space bar while you speak, release to send. A quick tap also works.' },
		{ id: 'toggle', label: 'Tap to start and stop', help: 'Tap the mic button (or press Space) to start, tap again to send.' },
		{ id: 'wake_word', label: 'Hands-free (wake word)', help: 'Start listening once, then just say the wake phrase and your request.' }
	];
	const LANGUAGES: { id: VoiceLanguage; label: string }[] = [
		{ id: 'en', label: 'English' },
		{ id: 'auto', label: 'Auto-detect' }
	];

	async function save() {
		saving = true;
		feedback = null;
		try {
			// Only the three known fields: the server refuses unknown ones.
			const { read_aloud, input_mode, language } = draft.value;
			const next = await settings.update({ voice: { read_aloud, input_mode, language } });
			draft.adopt(next.voice);
			feedback = { kind: 'ok', text: 'Voice settings saved.' };
		} catch (e) {
			feedback = { kind: 'error', text: describeError(toApiError(e)) };
		} finally {
			saving = false;
		}
	}
</script>

<SettingsSection
	id="voice"
	title="Voice"
	dirty={draft.dirty}
	{saving}
	{feedback}
	onsave={save}
	onrevert={() => draft.revert()}
>
	<div>
		<label class="flex items-start gap-3 text-sm text-ink">
			<input
				type="checkbox"
				class="mt-0.5 size-4 accent-[var(--color-accent)]"
				bind:checked={draft.value.read_aloud}
				disabled={!ttsAvailable}
				data-testid="read-aloud"
			/>
			<span>
				Read replies aloud
				<span class="mt-0.5 block text-xs text-mute">
					{ttsAvailable
						? `The assistant speaks its replies in the active persona's voice. Replies to spoken requests are always read.`
						: 'Speech output is not available on this server.'}
				</span>
			</span>
		</label>
	</div>

	<fieldset class="space-y-2">
		<legend class="hud-title mb-1">Input mode</legend>
		{#each MODES as m (m.id)}
			<label
				class="flex cursor-pointer items-start gap-3 border p-3 text-sm {draft.value.input_mode === m.id
					? 'border-accent bg-accent/10'
					: 'border-line bg-panel/60'}"
			>
				<input
					type="radio"
					name="input-mode"
					class="mt-0.5 size-4 accent-[var(--color-accent)]"
					value={m.id}
					bind:group={draft.value.input_mode}
					disabled={!sttAvailable && m.id !== 'hold'}
					data-testid="mode-{m.id}"
				/>
				<span>
					<span class="text-ink">{m.label}</span>
					<span class="mt-0.5 block text-xs text-mute">{m.help}</span>
				</span>
			</label>
		{/each}
		{#if !sttAvailable}
			<p class="text-xs text-warn"><span aria-hidden="true">△ </span>Speech recognition is not available on this server, so voice input is off.</p>
		{/if}
	</fieldset>

	{#if draft.value.input_mode === 'wake_word'}
		<div class="space-y-2 border border-line bg-panel/60 p-3 text-xs text-mute" data-testid="handsfree-help">
			<p class="font-medium text-ink">How hands-free works</p>
			<p>
				On the main screen, press <strong class="text-ink">Start listening</strong>. The microphone then stays open in that tab,
				with a clear indicator and a Stop button, until you stop it or leave the page. Say the wake phrase, then your request:
				{#each persona.list as p, i (p.id)}
					<span class="num text-ink">“{p.wake_word}”</span>{i < persona.list.length - 1 ? ' or ' : '.'}
				{/each}
				Saying another persona's phrase switches to that persona.
			</p>
			<p>
				<strong class="text-ink">Privacy.</strong> Audio is analysed in your browser, and a segment is sent to your local Arc MT
				server only when speech is detected. The server transcribes each segment to look for the wake phrase; segments without
				it are dropped and nothing is acted on. The audit log keeps transcripts, never audio.
			</p>
			<p>
				It pauses while the assistant is speaking. While a confirmation is open it ignores everything except
				“cancel”, and only trades on “<span class="num">{persona.wakeWord}</span>, confirm”.
			</p>
		</div>
	{/if}

	<div class="max-w-xs">
		<label for="voice-language" class="hud-title block">Language</label>
		<select id="voice-language" class="input mt-1" bind:value={draft.value.language}>
			{#each LANGUAGES as l (l.id)}
				<option value={l.id}>{l.label}</option>
			{/each}
		</select>
		<p class="mt-1 text-xs text-mute">Used for speech recognition. Auto-detect is slower and a little less exact.</p>
	</div>
</SettingsSection>
