/**
 * A banner's sentence pads its first line into the row the controls centre in,
 * and only above it. Padding below adds height to a wrapped bar. That pushed
 * the mobile header down and moved the file search panel off the screen top.
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
const shellCss: string = readFileSync(resolve(here, '../panels/shell.css'), 'utf-8');

const SENTENCES = ['backup-reminder-text', 'slowness-banner-text', 'ingress-banner-text', 'refusal-banner-text'];

describe('banner sentence padding', () => {
  it.each(SENTENCES)('%s pads its first line from above only', (sentence) => {
    const props = rulesTargeting(shellCss, sentence).flatMap(rule => [...rule.props.keys()]);
    expect(props).toContain('padding-top');
    expect(props).toContain('min-height');
    for (const forbidden of ['padding', 'padding-block', 'padding-bottom']) {
      expect(props, `${sentence} sets ${forbidden}, which pads below its last line`)
        .not.toContain(forbidden);
    }
  });
});
