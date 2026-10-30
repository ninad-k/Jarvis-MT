/** The server accepts at most 600 characters per speak request. */
export const MAX_SPEAK = 600;

/** Clip to the server limit at a sentence or word boundary. */
export function clipForSpeech(text: string, max = MAX_SPEAK): string {
	const t = text.replace(/\s+/g, ' ').trim();
	if (t.length <= max) return t;
	const cut = t.slice(0, max);
	const sentence = Math.max(cut.lastIndexOf('. '), cut.lastIndexOf('? '), cut.lastIndexOf('! '));
	if (sentence > max * 0.5) return cut.slice(0, sentence + 1);
	const space = cut.lastIndexOf(' ');
	return (space > 0 ? cut.slice(0, space) : cut).trimEnd();
}
