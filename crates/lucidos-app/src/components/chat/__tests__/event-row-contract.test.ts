import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const code = (src: string): string =>
  src
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('\n')
    .filter((l) => !/^\s*(\/\/|\*)/.test(l))
    .join('\n');

describe('event row contract', () => {
  const css = read('../../../styles/chat/event-rows.css');
  const row = read('../EventRow.tsx');
  const parts = read('../chat-exchange-parts.tsx');
  const child = read('../ChildCompletionRow.tsx');

  function block(selector: string): string {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
    return [...css.matchAll(re)].map((m) => m[1]).join('\n');
  }

  it('never truncates the subject', () => {
    const subject = block('.event-row-subject');
    expect(subject).toContain('overflow-wrap');
    expect(subject).not.toContain('white-space: nowrap');
    expect(subject).not.toContain('text-overflow');
  });

  it('wraps the facts line rather than clipping it', () => {
    const meta = block('.event-row-meta');
    expect(meta).toContain('flex-wrap: wrap');
    expect(meta).not.toContain('text-overflow');
  });

  it('draws no mark column', () => {
    expect(code(row)).not.toContain('event-row-mark');
    expect(code(css)).not.toContain('event-row-mark');
  });

  it.each([
    ['EventRow.tsx', () => row],
    ['chat-exchange-parts.tsx event-wait row', () => parts.slice(parts.indexOf('export function eventWaitRowBody'))],
    ['ChildCompletionRow.tsx', () => child],
  ])('%s takes no step outcome', (_name, source) => {
    const src = code(source());
    expect(src).not.toContain('stepStatus');
    expect(src).not.toContain('inline-step');
    expect(src).not.toContain('step-icon');
  });

  it('defines exactly one event-name chip', () => {
    expect([...css.matchAll(/^\.event-name\s*\{/gm)]).toHaveLength(1);
    expect(code(css)).not.toContain('.event-delivery-name');
  });

  it('tints every state from a token', () => {
    const tones = [...css.matchAll(/\.event-row-state\[data-tone="[a-z]+"\]\s*\{([^}]*)\}/g)];
    expect(tones.length).toBeGreaterThanOrEqual(6);
    for (const [, body] of tones) {
      expect(body).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
      expect(body).toMatch(/var\(--/);
    }
  });

  it('adds no left accent stripe', () => {
    expect(css).not.toMatch(/border-left\s*:/);
    expect(css).not.toMatch(/box-shadow:\s*inset/);
  });

  it('is a card, and a lighter one than .step-note-card', () => {
    const row = block('.event-row');
    expect(row).toContain('background: var(--bg-secondary)');
    expect(row).toContain('border-radius');
    expect(row).not.toContain('background: var(--bg-tertiary)');
  });

  it('puts the state on the subject line, on its first baseline', () => {
    const head = block('.event-row-head');
    expect(head).toContain('display: flex');
    expect(head).toContain('align-items: baseline');
    const state = block('.event-row-state');
    expect(state).not.toContain('align-self');
    expect(state).toContain('flex: 0 0 auto');
    expect(block('.event-row-subject')).toContain('min-width: 0');
  });

  it('draws the state as plain text, with no pill behind it', () => {
    const rules = [...css.matchAll(/\.event-row-state(\[data-tone="[a-z]+"\])?\s*\{([^}]*)\}/g)];
    expect(rules.length).toBeGreaterThanOrEqual(7);
    for (const [, , body] of rules) {
      expect(body).not.toMatch(/background|border-radius|padding/);
    }
  });

  it('keeps the state on the top line on a narrow pane', () => {
    expect(block('.event-row-head')).not.toContain('flex-wrap');
    expect(block('.event-row-subject')).toContain('flex: 1 1 0');
    const state = block('.event-row-state');
    expect(state).toMatch(/max-width:\s*\d+%/);
    expect(state).toContain('text-align: right');
  });

  it('draws the time above the card, right-aligned', () => {
    const time = block('.event-row-time');
    expect(time).toContain('display: block');
    expect(time).toContain('text-align: right');
    expect(code(row)).toMatch(/event-row-time[\s\S]*\{card\}/);
  });

  it('lifts the fold body off the card fill', () => {
    const fold = block('.event-row-fold-pre,\n.event-row-fold-body');
    expect(fold).toContain('background: var(--bg-tertiary)');
  });
});
