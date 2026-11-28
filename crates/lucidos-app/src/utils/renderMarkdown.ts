import { signal, type ReadonlySignal } from '@preact/signals';
import { escapeHtmlAttr } from './escapeHtmlAttr';
import type * as MarkdownEngine from './markdownEngine';

type Engine = typeof MarkdownEngine;

const engine = signal<Engine | null>(null);

export const markdownEngine: ReadonlySignal<Engine | null> = engine;

export function provideMarkdownEngine(loaded: Engine): void {
  engine.value = loaded;
}

const plainText = (md: string): string => escapeHtmlAttr(md).replace(/\n/g, '<br>');

export function renderMarkdown(md: string, opts?: Parameters<Engine['renderMarkdown']>[1]): string {
  return engine.value?.renderMarkdown(md, opts) ?? `<p>${plainText(md)}</p>`;
}

export function renderMarkdownInline(md: string): string {
  return engine.value?.renderMarkdownInline(md) ?? plainText(md);
}
