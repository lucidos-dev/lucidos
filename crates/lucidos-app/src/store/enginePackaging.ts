import { signal } from '@preact/signals';

/** The engine's packaging from /health. Null until health identifies the build. */
export const enginePackaged = signal<boolean | null>(null);
