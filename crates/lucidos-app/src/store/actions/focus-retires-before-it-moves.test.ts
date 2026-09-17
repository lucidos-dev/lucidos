/** `focusThread` retires the outgoing ride BEFORE it publishes the new focus.
 *
 *  A signal assignment runs its subscribers synchronously. So anything watching
 *  `focusedThreadId` acts while the old thread's *standing follow* is still
 *  armed and the shared transcript still holds its content. `watchCallLiveness`
 *  is such a watcher: a call already up on the INCOMING thread flips the
 *  transcript live inside `setFocusedThread`, and the wake behind that carries
 *  the OUTGOING thread's ride.
 *
 *  A SOURCE SCAN, because the hazard is an ordering rather than an output.
 *  Exercising it needs a live call, a focus change and a mounted transcript at
 *  once, and the reordered statements settle to the same state anyway. What a
 *  future refactor can silently undo is the order, so the order is what this
 *  reads.
 *
 *  Plan: `docs/plans/2026-09-16-a-call-carries-the-reader-to-the-live-edge.md`.
 */
import { describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join } from 'node:path';

const HERE = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(join(HERE, 'threads.ts'), 'utf8');

/** The body of `focusThread`, up to the next top-level declaration. */
function focusThreadBody(): string {
  const start = SOURCE.indexOf('export function focusThread(');
  expect(start, 'focusThread moved or was renamed').toBeGreaterThan(-1);
  const rest = SOURCE.slice(start + 1);
  const end = rest.indexOf('\nexport ');
  return end === -1 ? rest : rest.slice(0, end);
}

describe('focusThread retires the ride before it moves the focus', () => {
  it('calls stopFollowingBottom ahead of setFocusedThread', () => {
    const body = focusThreadBody();
    const retire = body.indexOf('stopFollowingBottom()');
    const publish = body.indexOf('setFocusedThread(');
    expect(retire, 'focusThread no longer retires the standing follow').toBeGreaterThan(-1);
    expect(publish, 'focusThread no longer publishes the focus').toBeGreaterThan(-1);
    expect(retire).toBeLessThan(publish);
  });

  it('publishes the focus exactly once, so the order above is the whole story', () => {
    // A second write would let a subscriber run before the retire anyway, from
    // a line this test never reads.
    const calls = focusThreadBody().match(/setFocusedThread\(/g) ?? [];
    expect(calls).toHaveLength(1);
  });
});
