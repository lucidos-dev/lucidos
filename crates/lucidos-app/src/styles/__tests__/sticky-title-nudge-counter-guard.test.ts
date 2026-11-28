import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { rulesTargeting, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const primitive = read('../../utils/webkitRepaint.ts');
const header = read('../../components/layout/MobileAppHeader.tsx');
const threadView = read('../../components/chat/ThreadView.tsx');

const sheets: Array<[string, string]> = readdirSync(resolve(here, '../..'), { recursive: true })
  .filter((p: unknown): p is string => typeof p === 'string' && p.endsWith('.css'))
  .map((p: string) => [p, readFileSync(resolve(here, '../..', p), 'utf-8')] as [string, string]);

function literal(name: string): string {
  const m = new RegExp(`${name}\\s*=\\s*'([^']+)'`).exec(primitive);
  expect(m, `${name} not declared in utils/webkitRepaint.ts`).not.toBeNull();
  return m![1];
}

const SHIFT_PROP = literal('PINNED_SHIFT_PROP');
const PINNED_ATTR = literal('SCROLLER_PINNED_ATTR');

function transformRules(): Array<[string, CssRule]> {
  return sheets.flatMap(([path, css]) =>
    rulesTargeting(css, 'mobile-thread-title-row')
      .filter(rule => rule.props.has('transform'))
      .map(rule => [path, rule] as [string, CssRule]),
  );
}

describe('the sticky thread title undoes the repaint nudge', () => {
  it('subtracts the published shift in every transform set on the row', () => {
    const rules = transformRules();
    expect(rules.length).toBeGreaterThan(0);
    for (const [path, rule] of rules) {
      expect(rule.props.get('transform'), `${path}: ${rule.selector}`).toContain(`calc(-1 * var(${SHIFT_PROP}`);
    }
  });

  it('keeps the hide-on-scroll offset on `translate`, apart from the counter', () => {
    for (const [path, rule] of transformRules()) {
      expect(rule.props.get('transform'), `${path}: ${rule.selector}`).not.toContain('--mobile-header-offset');
      expect(rule.props.get('translate'), `${path}: ${rule.selector}`).toContain('var(--mobile-header-offset');
      expect(rule.props.get('transition') ?? '', `${path}: ${rule.selector}`).not.toMatch(/(^|,)\s*transform\b/);
    }
  });

  it('marks the row so the primitive can find it', () => {
    const tag = /<div\b[^>]*?mobile-thread-title-row[^>]*>/.exec(header);
    expect(tag, 'no opening tag carrying .mobile-thread-title-row').not.toBeNull();
    expect(tag![0]).toContain(PINNED_ATTR);
  });

  it('renders the row as a direct child of the scroller the nudge writes to', () => {
    const open = /<div[^>]*class="thread-content visible"[^>]*ref=\{areaRef\}[^>]*>/.exec(threadView);
    expect(open, 'no transcript container carrying ref={areaRef}').not.toBeNull();
    const after = threadView.slice(open!.index + open![0].length);
    const at = after.indexOf('<MobileThreadTitleBar');
    expect(at, 'MobileThreadTitleBar is not rendered inside the transcript').toBeGreaterThanOrEqual(0);
    expect(after.slice(0, at), 'a wrapper stands between the scroller and the row').not.toContain('<');
  });

  it('publishes the counter on that child alone, never on :root', () => {
    for (const [path, css] of sheets) {
      expect(css, path).not.toMatch(new RegExp(`:root[^{]*\\{[^}]*${SHIFT_PROP}`));
    }
  });
});
