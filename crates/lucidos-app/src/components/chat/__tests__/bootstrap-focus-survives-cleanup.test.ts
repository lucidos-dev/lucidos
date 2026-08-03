import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const threadView = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf-8');
const threads = readFileSync(resolve(here, '../../../store/actions/threads.ts'), 'utf-8');

/**
 * Two halves of one contract, which only work together.
 *
 * `focusThreadOrBootstrapResult` focuses a thread OPTIMISTICALLY when it isn't
 * in `threadMap` yet, so a notification tap navigating to a thread outside the
 * loaded window moves the pane at once and lands on ThreadView's existing
 * delay-gated skeleton, instead of the dead interval it used to have while the
 * metadata fetch ran. On a cold push tap the map is always empty, so that was
 * every single tap.
 *
 * But ThreadView clears a `focusedThreadId` whose thread isn't in the map, as
 * stale-pointer cleanup during render. An optimistically-focused thread is
 * absent from the map for exactly that reason, so without the
 * `bootstrappingThreadId` exemption the cleanup undoes the focus on the very
 * next render and the dead interval comes straight back, silently.
 *
 * Neither half is meaningful alone, and ThreadView is not render-tested (it
 * pulls the whole chat stack), so this is a source-scan tripwire in the same
 * shape as the other ThreadView invariants in this directory. The behavioural
 * assertions live in `store/actions/threads-ensure-status.test.ts`.
 */
describe('optimistic bootstrap focus survives ThreadView stale-pointer cleanup', () => {
  it('ThreadView exempts a bootstrapping thread from the unfocus cleanup', () => {
    // The cleanup must be gated on BOTH threadsLoaded and the exemption.
    expect(threadView).toMatch(
      /if\s*\(threadsLoaded\.value\s*&&\s*bootstrappingThreadId\.value\s*!==\s*threadId\)/,
    );
  });

  it('ThreadView reads the exemption from the store, not a local guess', () => {
    expect(threadView).toMatch(/import\s*\{[^}]*\bbootstrappingThreadId\b[^}]*\}\s*from\s*'\.\.\/\.\.\/store\/store'/s);
  });

  it('the bootstrap sets the flag and focuses before awaiting the metadata', () => {
    // Order matters: both must precede the `await ensureThreadByIdInMap`, or the
    // tap is unacknowledged for the whole round-trip.
    const miss = threads.slice(
      threads.indexOf('export async function focusThreadOrBootstrapResult'),
      threads.indexOf('await ensureThreadByIdInMap'),
    );
    expect(miss).toContain('bootstrappingThreadId.value = threadId');
    expect(miss).toContain('setFocusedThread(threadId)');
    expect(miss).toContain('revealThreadPane()');
  });

  it('every non-focused exit from the bootstrap releases the flag', () => {
    // A leaked flag would exempt a genuinely stale pointer from cleanup forever.
    expect(threads).toMatch(/catch \(error\) \{\s*releaseBootstrap\(threadId, previousFocus\);/);
    expect(threads).toMatch(/if \(!found\) \{\s*releaseBootstrap\(threadId, previousFocus\);/);
    expect(threads).toMatch(/if \(bootstrappingThreadId\.value === threadId\) bootstrappingThreadId\.value = null;/);
  });
});
