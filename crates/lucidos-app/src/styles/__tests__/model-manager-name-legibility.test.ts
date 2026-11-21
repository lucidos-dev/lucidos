/**
 * A model is identified by its name AND its id, so a Settings → Models row that
 * clips either one identifies nothing.
 *
 * Both lines used to be `white-space: nowrap` with an ellipsis, against a row
 * where the info column was the only flexible child: beside it sit a provider
 * chip, an optional "not set up" chip, and the builtin marker or a Delete
 * button. At phone width that column got about a third of the row. So
 * "Nemotron 3.5 Lightning (free)" read as "Nemotron 3.5…" and its id as
 * "nemotron-3.5-l…". The row wraps now instead of squeezing its text.
 *
 * A source scan rather than a browser test, and `rulesTargeting` rather than
 * `block`: the assertion is that NOTHING anywhere in the sheet re-clips these
 * two, which a first-textual-match reader cannot answer.
 */
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

/** The values that stop a line wrapping. `pre-wrap` and `pre-line` do wrap. */
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
    // `break-word` would not do: it breaks the token but is not counted when
    // the flex line is measured, so the row still overflows its pane.
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
