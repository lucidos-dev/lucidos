/** `CreateThreadView` hands a turn that draws the same under either flag value
 *  a fixed `threadIdle` and `threadAwaitingAnswer` (`threadFlagsChangeDrawing`).
 *  The shortcut holds while `ChatExchange` reads the flags only through the
 *  three functions the check compares, and the queued hold it treats as always
 *  sensitive. A new read goes stale under the fixed value. */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const SOURCE: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf8');

/** Lines reading a flag, minus the prop declarations and the memo compare. */
function readsOf(flag: string): string[] {
  return SOURCE.split('\n')
    .map((line) => line.trim())
    .filter((line) => new RegExp(`\\b${flag}\\b`).test(line))
    .filter((line) => !line.startsWith('*') && !line.startsWith('//'))
    .filter((line) => !line.startsWith(`${flag}:`) && !line.includes(`prev.${flag}`) && !line.startsWith('function ChatExchangeImpl'));
}

describe('ChatExchange reads of the thread flags', () => {
  it('reads threadIdle only through the three compared functions', () => {
    const reads = readsOf('threadIdle');
    expect(reads).toHaveLength(3);
    // `drawnResponseEvents` filters `exchangeResponseEvents` by a flag-blind switch.
    expect(reads.filter((line) => /(exchange(Steps|ResponseEvents|Status)|drawnResponseEvents)\(/.test(line))).toHaveLength(3);
  });

  it('reads threadAwaitingAnswer through exchangeStatus and the queued hold only', () => {
    const reads = readsOf('threadAwaitingAnswer');
    expect(reads).toHaveLength(2);
    expect(reads.some((line) => line.includes('exchangeStatus('))).toBe(true);
    expect(reads.some((line) => line.includes('isQueuedUserMessage && threadAwaitingAnswer'))).toBe(true);
  });
});
