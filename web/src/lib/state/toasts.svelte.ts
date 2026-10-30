export type ToastKind = 'info' | 'error';

export interface Toast {
	id: number;
	kind: ToastKind;
	text: string;
}

/** Short-lived messages for failures that have no inline home (a close button, a dropped socket). */
class ToastStore {
	items = $state<Toast[]>([]);
	private next = 1;

	push(text: string, kind: ToastKind = 'info', ms = 6000): void {
		// Same text twice in a row would only stack noise.
		if (this.items.some((t) => t.text === text)) return;
		const id = this.next++;
		this.items.push({ id, kind, text });
		setTimeout(() => this.dismiss(id), ms);
	}

	error(text: string): void {
		this.push(text, 'error', 8000);
	}

	dismiss(id: number): void {
		this.items = this.items.filter((t) => t.id !== id);
	}
}

export const toasts = new ToastStore();
