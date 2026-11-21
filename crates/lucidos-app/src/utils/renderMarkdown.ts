/**
 * The markdown facade. The markdown engine (`markdownEngine.ts`, with `marked`
 * and DOMPurify) is its own chunk, so the first frame never waits for it.
 *
 * Until `provideMarkdownEngine` runs, each call returns its input as escaped
 * plain text. A component that calls one while it renders reads
 * `markdownEngine`, so it draws again, formatted, once the engine lands. A
 * caller that memoises the output lists `markdownEngine.value` in its
 * dependencies. Nothing here caches the plain text.
 */
import { signal, type ReadonlySignal } from '@preact/signals';
import { escapeHtmlAttr } from './escapeHtmlAttr';
import type * as MarkdownEngine from './markdownEngine';

type Engine = typeof MarkdownEngine;

const engine = signal<Engine | null>(null);

/** The loaded engine, or null before it lands. */
export const markdownEngine: ReadonlySignal<Engine | null> = engine;

export function provideMarkdownEngine(loaded: Engine): void {
  engine.value = loaded;
}

/** Every character of `md` as text, with its line breaks kept. */
const plainText = (md: string): string => escapeHtmlAttr(md).replace(/\n/g, '<br>');

export function renderMarkdown(md: string, opts?: Parameters<Engine['renderMarkdown']>[1]): string {
  return engine.value?.renderMarkdown(md, opts) ?? `<p>${plainText(md)}</p>`;
}

export function renderMarkdownInline(md: string): string {
  return engine.value?.renderMarkdownInline(md) ?? plainText(md);
}
