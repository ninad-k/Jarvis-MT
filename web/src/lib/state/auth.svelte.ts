import { goto } from '$app/navigation';
import { api, setUnauthorizedHandler, type LoginRequest } from '#lib/api.ts';
import { ApiError, toApiError } from '#lib/errors.ts';
import { status } from './hud.svelte';

/**
 * Session state. The session cookie is HttpOnly, so the client never sees a token: it only
 * reacts to 401 responses and asks /api/auth/me who it is.
 */
class AuthStore {
	/** Username from /api/auth/me or the last login; null when signed out or auth is off. */
	user = $state<string | null>(null);
	busy = $state(false);

	/** Whether the server requires sign-in; null until the first status reply. */
	get enabled(): boolean | null {
		return status.data?.auth ? status.data.auth.enabled : null;
	}

	get username(): string | null {
		return this.user ?? status.data?.auth?.username ?? null;
	}

	/** Look the session up. Any failure other than 401 (e.g. a server without auth) is ignored. */
	async refresh(): Promise<void> {
		try {
			this.user = (await api.me()).username;
		} catch (e) {
			if (e instanceof ApiError && e.isUnauthorized && this.enabled !== false) this.expire();
		}
	}

	/** Throws ApiError: `unauthorized` for wrong credentials, `rate_limited` with retryAfterMs. */
	async login(body: LoginRequest): Promise<void> {
		this.busy = true;
		try {
			const s = await api.login(body);
			this.user = s.username;
		} catch (e) {
			throw toApiError(e);
		} finally {
			this.busy = false;
		}
	}

	async logout(): Promise<void> {
		try {
			await api.logout();
		} catch {
			/* the cookie may already be gone; leave either way */
		}
		this.expire();
	}

	/** The server said 401 (or the user logged out): clear state and go to /login. */
	expire(): void {
		this.user = null;
		// Several requests can fail together; only the first one navigates. The login page
		// clears the cached data (see `resetPollers`) once the HUD's pollers have stopped.
		if (location.pathname !== '/login') void goto('/login', { replace: true });
	}
}

export const auth = new AuthStore();

setUnauthorizedHandler(() => auth.expire());
