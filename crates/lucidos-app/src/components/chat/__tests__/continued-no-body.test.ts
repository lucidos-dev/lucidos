import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');
const partsSource = readFileSync(resolve(here, '../chat-exchange-parts.tsx'), 'utf-8');

describe('Empty Continued-below panel is hidden entirely', () => {
  it('ChatExchange skips the response panel for empty non-last done/interrupted exchanges', () => {
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    expect(fnMatch, 'ChatExchangeImpl function not found').not.toBeNull();
    const fn = fnMatch![0];
    expect(fn).toMatch(/isEmptyContinued\s*=\s*isEmptyContinuedExchange\(status,\s*hasResponse,\s*events,\s*isLast\)/);
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*!isEmptyContinued/);
  });

  it('ResponsePanel still accepts hasBody and opens .response-body on it', () => {
    expect(partsSource).toMatch(/interface ResponsePanelProps[\s\S]*?hasBody:\s*boolean/);
    const fnMatch = partsSource.match(/function ResponsePanel\(\{[\s\S]*?\n\}\n/);
    expect(fnMatch, 'ResponsePanel function not found').not.toBeNull();
    expect(fnMatch![0]).toMatch(/<Disclosure open=\{hasBody && !folded\}>\s*<div class="response-body">/);
    expect(fnMatch![0]).not.toMatch(/hasBody\s*&&\s*\(\s*<Disclosure/);
  });

  it('ChatExchange passes a real content test to ResponsePanel, not a constant', () => {
    expect(source).toMatch(/<ResponsePanel[\s\S]*?hasBody=\{canCollapse\}/);
    expect(source).toMatch(/const canCollapse = hasResponse \|\| events\.some\(/);
  });
});
