/**
 * The part grammar twin agrees with the engine, case for case, and every apply
 * site runs it on part tokens (ADR 0307).
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { checkPartToken } from './themeParts';
import { parseResolvedTheme, parseStyleOverrides } from './appearance';

const here: string = dirname(fileURLToPath(import.meta.url));
const cases: {
  valid: { token: string; value: string; canonical: string }[];
  invalid: { token: string; value: string; error: string }[];
} = JSON.parse(readFileSync(
  resolve(here, '../../../crates/lucidos-engine/src/core/themes/theme-part-cases.json'),
  'utf-8',
));

describe('the part grammar agrees with the engine', () => {
  // The engine's `parts_tests.rs` reads the same fixture.
  it.each(cases.valid)('accepts $token = $value', ({ token, value, canonical }) => {
    expect(checkPartToken(token, value)).toEqual({ ok: canonical });
  });

  it.each(cases.invalid)('refuses $token = $value', ({ token, value, error }) => {
    expect(checkPartToken(token, value)).toEqual({ error });
  });
});

describe('every apply site checks part tokens', () => {
  it('drops a style override part token past a cap and keeps a good one canonical', () => {
    const map = parseStyleOverrides(JSON.stringify({
      '--part-chat-text-text-shadow': '0 0 9em red',
      '--part-header-title-letter-spacing': '0.05EM',
      '--accent': '#ff0000',
    }));
    expect(map).toEqual({ '--part-header-title-letter-spacing': '0.05em', '--accent': '#ff0000' });
  });

  it('drops a part token in a resolved theme that did not come from the engine', () => {
    const theme = parseResolvedTheme(JSON.stringify({
      dark: { '--part-actor-icon-filter': 'blur(20px)', '--part-chat-text-color': '#33ff33' },
      light: { '--part-unknown-color': 'red' },
    }));
    expect(theme.dark).toEqual({ '--part-chat-text-color': '#33ff33' });
    expect(theme.light).toEqual({});
  });
});
