import { alerts } from './alerts.svelte';
import { status, watchlist } from './hud.svelte';
import { settings } from './settings.svelte';

/**
 * Re-read everything a preference change can touch: the persona toggle and model menu (settings
 * and status), the watchlist rows, the limits shown in Settings, the kill switch (status) and the
 * alerts. Called on the `settings_changed` event and after a chat reply that used a preference
 * tool, so the HUD is right even when a push frame is missed.
 */
export function refreshPreferences(): void {
	void settings.refresh();
	settings.bumpLimits();
	status.refresh();
	watchlist.refresh();
	void alerts.refresh();
}
