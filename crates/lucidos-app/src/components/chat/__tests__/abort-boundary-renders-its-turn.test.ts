import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

describe('an abort or cancel boundary renders a turn that landed under it', () => {
  it('showResponsePanel admits a terminated boundary that has content', () => {
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    expect(fnMatch, 'ChatExchangeImpl function not found').not.toBeNull();
    const fn = fnMatch![0];

    expect(fn).toMatch(
      /isTerminatedContinuation\s*=\s*\(isAbortPanel \|\| isCancelPanel\)\s*&&\s*\(hasResponse \|\| hasRenderableResponseContent\(events\)\)/,
    );
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*\(!isAbortPanel \|\| isTerminatedContinuation\)/);
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*\(!isCancelPanel \|\| isTerminatedContinuation\)/);
  });

  it('does not gate the exception on the bare event count', () => {
    const fn = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m)![0];
    const stmt = fn.match(/const isTerminatedContinuation[\s\S]*?;/)!;
    expect(stmt[0]).not.toContain('hasEvents');
  });

  it('does not simply drop the boundary exclusions', () => {
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    const fn = fnMatch![0];
    const line = fn.split('\n').find((l: string) => l.includes('const showResponsePanel'))!;
    expect(line).toContain('!isAbortPanel || isTerminatedContinuation');
    expect(line).toContain('!isCancelPanel || isTerminatedContinuation');
  });
});
