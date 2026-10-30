interface InstallPromptEvent extends Event {
	prompt(): Promise<void>;
	userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>;
}

/** Install prompt for the PWA. Chromium fires `beforeinstallprompt`; iOS Safari never does. */
class PwaStore {
	private event = $state<InstallPromptEvent | null>(null);
	installed = $state(false);

	get canInstall(): boolean {
		return this.event !== null && !this.installed;
	}

	/** iOS Safari has no install API: the menu shows the manual steps instead. */
	get iosHint(): boolean {
		if (typeof navigator === 'undefined' || this.installed) return false;
		const ios = /iphone|ipad|ipod/i.test(navigator.userAgent);
		return ios && !this.standalone();
	}

	standalone(): boolean {
		return (
			matchMedia('(display-mode: standalone)').matches ||
			(navigator as unknown as { standalone?: boolean }).standalone === true
		);
	}

	/** Start listening. Returns the cleanup. */
	attach(): () => void {
		this.installed = this.standalone();
		const before = (e: Event) => {
			e.preventDefault();
			this.event = e as InstallPromptEvent;
		};
		const done = () => {
			this.installed = true;
			this.event = null;
		};
		window.addEventListener('beforeinstallprompt', before);
		window.addEventListener('appinstalled', done);
		return () => {
			window.removeEventListener('beforeinstallprompt', before);
			window.removeEventListener('appinstalled', done);
		};
	}

	async install(): Promise<void> {
		const e = this.event;
		if (!e) return;
		this.event = null; // the browser allows one prompt per event
		await e.prompt();
		const { outcome } = await e.userChoice;
		if (outcome === 'accepted') this.installed = true;
	}
}

export const pwa = new PwaStore();
