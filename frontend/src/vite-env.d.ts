/// <reference types="svelte" />
/// <reference types="vite/client" />

declare namespace svelteHTML {
	interface HTMLAttributes {
		// Svelte only types this on <input>; Safari uses it on contenteditable too.
		autocorrect?: 'on' | 'off' | '' | undefined | null;
	}
}
