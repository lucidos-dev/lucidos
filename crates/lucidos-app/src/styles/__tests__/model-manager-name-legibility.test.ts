import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const settingsCss = readFileSync(resolve(here, '../settings/base.css'), 'utf-8');

const NON_WRAPPING = ['nowrap', 'pre'];

describe('Settings → Models row legibility', () => {
  for (const cls of ['model-manager-name', 'model-manager-id']) {
    it(`.${cls} is never clipped by any rule`, () => {
      const rules = rulesTargeting(settingsCss, cls);
      expect(rules.length, `no rule targets .${cls}`).toBeGreaterThan(0);
      for (const rule of rules) {
        expect(rule.props.get('text-overflow'), `${rule.selector} ellipsizes`)
          .not.toBe('ellipsis');
        const ws = rule.props.get('white-space');
        if (ws) expect(NON_WRAPPING, `${rule.selector} sets white-space: ${ws}`)
          .not.toContain(ws);
      }
    });
  }

  it('the id can break mid-token, since it is one unbroken word', () => {
    const rules = rulesTargeting(settingsCss, 'model-manager-id');
    const wrap = rules.map(r => r.props.get('overflow-wrap')).filter(Boolean);
    expect(wrap).toContain('anywhere');
  });

  it('the row wraps, so the metadata cluster drops below a long name', () => {
    const row = rulesTargeting(settingsCss, 'model-manager-row')
      .find(r => r.props.has('flex-wrap'));
    expect(row?.props.get('flex-wrap')).toBe('wrap');
  });

  it('the metadata cluster wraps as one unit, not chip by chip', () => {
    const meta = rulesTargeting(settingsCss, 'model-manager-meta')
      .find(r => r.props.has('display'));
    expect(meta?.props.get('display')).toBe('flex');
  });
});
