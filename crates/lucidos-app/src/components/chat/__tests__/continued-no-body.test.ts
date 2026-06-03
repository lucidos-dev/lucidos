import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
// ChatExchangeImpl + its <ResponsePanel> usage live in ChatExchange.tsx;
// the ResponsePanel component itself moved to chat-exchange-parts.tsx.
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');
const partsSource = readFileSync(resolve(here, '../chat-exchange-parts.tsx'), 'utf-8');

/**
 * A non-last CC exchange that produced nothing (no response, no events) is
 * pure visual noise — the entire response panel is hidden, not just its body.
 * The next exchange's user message implies the chronological flow without
 * needing a "Continued below ↳" placeholder.
 *
 * Both terminal-ish statuses qualify: 'done' (clean handoff — CC went idle or
 * the session ended normally) and 'interrupted' (mid-work follow-up — CC had a
 * step like SessionStarted but emitted no visible events before the next user
 * message landed). Without 'interrupted', SessionStarted-only exchanges still
 * render an empty header.
 *
 * The latest exchange is always shown — the !isLast guard means an active
 * Claude Code session (working/streaming/pending) keeps its panel even before any
 * events arrive.
 *
 * ResponsePanel still gates its body div on hasBody for the rare cases
 * where a panel renders without body content (queued, pending-with-no-
 * events-yet, error states).
 */
describe('Empty Continued-below panel is hidden entirely', () => {
  it('ChatExchange skips the response panel for empty non-last done/interrupted exchanges', () => {
    // ChatExchange is `memo(ChatExchangeImpl, …)`; the body to scan is the
    // `function ChatExchangeImpl(...)` declaration.
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    expect(fnMatch, 'ChatExchangeImpl function not found').not.toBeNull();
    const fn = fnMatch![0];
    // showResponsePanel must rule out the empty non-last terminal-ish cases —
    // delegated to isEmptyContinuedExchange (in thread-events.ts) so the
    // Thinking-only exception is shared with tests.
    expect(fn).toMatch(/isEmptyContinued\s*=\s*isEmptyContinuedExchange\(status,\s*hasResponse,\s*events,\s*isLast\)/);
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*!isEmptyContinued/);
  });

  it('ResponsePanel still accepts hasBody and gates .response-body on it', () => {
    expect(partsSource).toMatch(/interface ResponsePanelProps[\s\S]*?hasBody:\s*boolean/);
    const fnMatch = partsSource.match(/function ResponsePanel\(\{[\s\S]*?\n\}\n/);
    expect(fnMatch, 'ResponsePanel function not found').not.toBeNull();
    expect(fnMatch![0]).toMatch(/hasBody\s*&&\s*!collapsed[\s\S]*?class="response-body"/);
  });

  it('ChatExchange passes hasBody=hasResponse||hasEvents to ResponsePanel', () => {
    expect(source).toMatch(/<ResponsePanel[\s\S]*?hasBody=\{hasResponse \|\| hasEvents\}/);
  });
});
