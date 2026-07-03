import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import type { Exchange } from '../../../store/thread-events';
import { exchangeEngineLimitDetail } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');
// settings.css is an @import barrel; inline the partials it pulls in so the
// rule assertions below see the full concatenated stylesheet regardless of
// which partial a rule lives in.
const settingsBarrel = readFileSync(resolve(here, '../../../styles/settings.css'), 'utf-8');
const css = settingsBarrel.replace(
  /@import\s+'([^']+)';/g,
  (_m: string, rel: string) => readFileSync(resolve(here, '../../../styles', rel), 'utf-8'),
);

/**
 * When the chat agent hits the per-turn tool-call cap, the engine emits a
 * ResponseGenerated whose text starts with "[ENGINE-LIMIT]". The cap arrives
 * with NO preceding TextStreamed — so the prefix never lands in the streamed
 * text concatenation. Without a dedicated side channel that reads
 * ResponseGenerated.text directly, the agent appears to silently stop mid-task.
 *
 * The fix has two halves:
 * 1. `exchangeEngineLimitDetail(exchange)` scans steps for the latest
 *    ResponseGenerated and returns the "[ENGINE-LIMIT] …" body (minus the
 *    prefix) — or empty string. This is the side channel.
 * 2. ChatExchange renders an .exchange-engine-limit yellow banner OUTSIDE
 *    both the (hasSections) and (!hasSections) render branches, so the
 *    banner appears at the bottom of the response panel regardless of how
 *    many tool steps are above it (cap fires at iteration N+1 with N
 *    successful steps — hasEvents is always true in that case).
 */
describe('ENGINE-LIMIT cap detection helper', () => {
  function fakeExchange(steps: Array<{ event: { type: string; text?: string } }>): Exchange {
    return { steps } as unknown as Exchange;
  }

  it('returns the message body when ResponseGenerated carries the prefix', () => {
    const ex = fakeExchange([
      { event: { type: 'ToolCalled' } },
      { event: { type: 'ResponseGenerated', text: '[ENGINE-LIMIT] Per-turn limit of 500 tool calls reached. Send any message to continue from here.' } },
    ]);
    expect(exchangeEngineLimitDetail(ex)).toBe('Per-turn limit of 500 tool calls reached. Send any message to continue from here.');
  });

  it('returns empty string when ResponseGenerated text lacks the prefix', () => {
    const ex = fakeExchange([
      { event: { type: 'ResponseGenerated', text: 'A normal response.' } },
    ]);
    expect(exchangeEngineLimitDetail(ex)).toBe('');
  });

  it('returns empty string when there is no ResponseGenerated', () => {
    const ex = fakeExchange([
      { event: { type: 'ToolCalled' } },
      { event: { type: 'TextStreamed', text: '[ENGINE-LIMIT] decoy' } },
    ]);
    expect(exchangeEngineLimitDetail(ex)).toBe('');
  });

  it('considers only the latest ResponseGenerated', () => {
    const ex = fakeExchange([
      { event: { type: 'ResponseGenerated', text: '[ENGINE-LIMIT] earlier' } },
      { event: { type: 'ResponseGenerated', text: 'fresher response, no cap' } },
    ]);
    expect(exchangeEngineLimitDetail(ex)).toBe('');
  });
});

describe('ENGINE-LIMIT banner', () => {
  it('detects the cap via exchangeEngineLimitDetail, not via streamed text', () => {
    // The cap arrives only on ResponseGenerated.text, which exchangeResponseText
    // does NOT read. The component must call the dedicated helper instead of
    // grepping responseTextRaw.
    expect(source).toMatch(/engineLimitDetail\s*=\s*!streamingBuffer\s*\?\s*exchangeEngineLimitDetail\(exchange\)\s*:\s*['"]['"]/);
    expect(source).toMatch(/isEngineLimit\s*=\s*!!engineLimitDetail/);
    expect(source).not.toMatch(/responseTextRaw\.startsWith\(['"]\[ENGINE-LIMIT\]['"]\)/);
  });

  it('renders the banner outside the hasSections / hasEvents render branches', () => {
    // The banner must be a sibling of the response-content ternary, not nested
    // inside the !hasEvents branch — the cap always fires with 100+ tool steps,
    // so hasEvents is always true when isEngineLimit is true.
    //
    // Asserted by locating the banner JSX and confirming the closing `</div>` +
    // closing `)}` of the response-content ternary appears immediately before it
    // — i.e. the banner is at the same nesting depth as the ternary, not inside.
    const bannerIdx = source.indexOf('{isEngineLimit && (');
    expect(bannerIdx, 'banner JSX not found').toBeGreaterThan(0);
    const before = source.slice(0, bannerIdx).trimEnd();
    expect(before.endsWith(')}')).toBe(true);
  });

  it('preserves the response panel even when there is no streamed text', () => {
    expect(source).toMatch(/hasResponse\s*=\s*!!responseHtmlCombined\s*\|\|\s*isEngineLimit/);
  });

  it('styles the banner with the yellow accent (warning, not error red)', () => {
    expect(css).toMatch(/\.exchange-engine-limit\s*\{[^}]*--accent-yellow/);
    expect(css).not.toMatch(/\.exchange-engine-limit\s*\{[^}]*--accent-red/);
  });
});
