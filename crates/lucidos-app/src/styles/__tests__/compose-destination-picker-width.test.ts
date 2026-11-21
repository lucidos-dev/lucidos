/**
 * The compose destination dropdown matches the prompt box width on a phone
 * only. There its label nearly spans the pane, so a content-width dropdown
 * ends just short of the prompt box's edge. On desktop it keeps its label's
 * width. The desktop half guards against a dropdown stretched across a wide
 * pane, and the phone half guards the stretch itself.
 */
import { describe, it, expect } from 'vitest';
import postcss, { type AtRule, type Rule } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const INPUT_CSS = resolve(here, '../chat/input-messages.css');

const ROW = '.compose-destination-row';
const PICKER = '.compose-destination-row .compose-destination-picker';
const TRIGGER = '.compose-destination-row .compose-destination-picker .dropdown-trigger';

type Layout = 'any' | 'phone';

/** The media query a rule sits under: none, or the phone layout. */
function layoutOf(rule: Rule): Layout | 'other' {
  const parent = rule.parent;
  if (parent?.type === 'root') return 'any';
  if (parent?.type === 'atrule' && (parent as AtRule).params.trim() === '(--phone-layout)') return 'phone';
  return 'other';
}

/** Declarations of one selector, from the rules under the given layout. */
function declarationsOf(selector: string, layout: Layout): Record<string, string> {
  const root = postcss.parse(readFileSync(INPUT_CSS, 'utf8'));
  const out: Record<string, string> = {};
  root.walkRules((rule: Rule) => {
    if (rule.selector.trim() !== selector || layoutOf(rule) !== layout) return;
    rule.walkDecls((d) => {
      out[d.prop] = d.value.trim();
    });
  });
  return out;
}

describe('the compose destination picker on desktop', () => {
  it('keeps the row at its content width', () => {
    const row = declarationsOf(ROW, 'any');
    expect(row['width']).toBeUndefined();
    expect(row['align-self']).toBeUndefined();
  });

  it('does not grow the picker or its trigger', () => {
    expect(declarationsOf(PICKER, 'any')['flex']).toBeUndefined();
    expect(declarationsOf(TRIGGER, 'any')['width']).toBeUndefined();
  });
});

describe('the compose destination picker on a phone', () => {
  it('stretches the row to the wrapper\'s full width', () => {
    const row = declarationsOf(ROW, 'phone');
    expect(row['width']).toBe('100%');
    expect(row['align-self']).toBe('stretch');
  });

  it('grows the picker to fill the row instead of its label width', () => {
    expect(declarationsOf(PICKER, 'phone')['flex']).toBe('1 1 auto');
  });

  it('stretches the trigger button to fill the picker', () => {
    expect(declarationsOf(TRIGGER, 'phone')['width']).toBe('100%');
  });

  it('leaves the coding-agent chip beside it at its own content width', () => {
    // So the picker, not the chip, is what absorbs the row's stretch.
    expect(declarationsOf('.compose-coding-agent-chip', 'any')['flex-shrink']).toBe('0');
  });
});
