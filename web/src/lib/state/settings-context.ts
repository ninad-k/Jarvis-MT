import { getContext, setContext } from 'svelte';
import type { DirtyRegistry } from './draft.svelte';

const KEY = Symbol('arcmt.settings.dirty');

export const provideDirtyRegistry = (r: DirtyRegistry) => setContext(KEY, r);
export const useDirtyRegistry = () => getContext<DirtyRegistry | undefined>(KEY);

/** The line a section shows after Save: what happened, in words. */
export interface Feedback {
	kind: 'ok' | 'error';
	text: string;
}
