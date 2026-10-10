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
const pluginInstall = readFileSync(resolve(here, '../../../store/actions/plugin-install.ts'), 'utf-8');
const storeTab = readFileSync(resolve(here, '../../plugins/StoreTab.tsx'), 'utf-8');

/**
 * Two halves of one contract, which only work together.
 *
 * A thread can be focused before this client has its row. Two producers reach
 * that state. `focusThreadOrBootstrapResult` focuses OPTIMISTICALLY while it
 * fetches the metadata, so a notification tap moves the pane at once instead of
 * sitting dead for the round-trip. `focusSpawnedThread` focuses a thread the
 * engine has just spawned, whose row arrives over SSE.
 *
 * But ThreadView clears a `focusedThreadId` whose thread isn't in the map, as
 * stale-pointer cleanup during render. Both producers are absent from the map
 * for that very reason. So without the `awaitedThreadId` exemption the cleanup
 * undoes the focus on the next render, and the user lands on the compose view.
 *
 * Neither half is meaningful alone, and ThreadView is not render-tested (it
 * pulls the whole chat stack), so this is a source-scan tripwire in the same
 * shape as the other ThreadView invariants in this directory. The behavioural
 * assertions live in `store/actions/threads-ensure-status.test.ts`.
 */
describe('an awaited focus survives ThreadView stale-pointer cleanup', () => {
  it('ThreadView exempts an awaited thread from the unfocus cleanup', () => {
    // The cleanup must be gated on BOTH threadsLoaded and the exemption.
    expect(threadView).toMatch(
      /if\s*\(threadsLoaded\.value\s*&&\s*awaitedThreadId\.value\s*!==\s*threadId\)/,
    );
  });

  it('ThreadView reads the exemption from the store, not a local guess', () => {
    expect(threadView).toMatch(/import\s*\{[^}]*\bawaitedThreadId\b[^}]*\}\s*from\s*'\.\.\/\.\.\/store\/store'/s);
  });

  it('the bootstrap sets the flag and focuses before awaiting the metadata', () => {
    // Order matters: both must precede the `await ensureThreadByIdInMap`, or the
    // tap is unacknowledged for the whole round-trip.
    const miss = threads.slice(
      threads.indexOf('export async function focusThreadOrBootstrapResult'),
      threads.indexOf('await ensureThreadByIdInMap'),
    );
    expect(miss).toContain('awaitedThreadId.value = threadId');
    expect(miss).toContain('setFocusedThread(threadId)');
    expect(miss).toContain('revealThreadPane()');
  });

  it('the bootstrap retires the standing follow at its OWN focus, not at the focusThread it ends with', () => {
    // The third thing that has to happen before the await, and the one the
    // optimistic focus quietly breaks. `focusThread` retires the follow only
    // when it is actually opening a different thread, and by the time this
    // function calls it the optimistic focus has ALREADY moved: that call reads
    // "same thread, nothing was left" and keeps the previous thread's follow
    // armed over the one being opened, which then rides a live edge nobody
    // asked for and records that borrowed request as its own reading position.
    // So the retire belongs at the optimistic focus, which is where this
    // navigation really leaves a thread.
    const miss = threads.slice(
      threads.indexOf('export async function focusThreadOrBootstrapResult'),
      threads.indexOf('await ensureThreadByIdInMap'),
    );
    expect(miss).toContain('stopFollowingBottom()');
  });

  it('every non-focused exit from the bootstrap releases the flag', () => {
    // A leaked flag would exempt a genuinely stale pointer from cleanup forever.
    expect(threads).toMatch(/catch \(error\) \{\s*releaseAwait\(threadId, previousFocus\);/);
    expect(threads).toMatch(/if \(!found\) \{\s*releaseAwait\(threadId, previousFocus\);/);
    expect(threads).toMatch(/if \(awaitedThreadId\.value === threadId\) awaitedThreadId\.value = null;/);
  });

  it('ThreadView releases the await once the thread lands in the map', () => {
    // The other half of the same leak. ThreadView is the exemption's only
    // reader, so it is where the arrival is noticed. A clear at each map-insert
    // site instead would drift the next time one is added.
    expect(threadView).toMatch(
      /threadInMap\s*&&\s*awaitedThreadId\.value === threadId\)\s*\{\s*awaitedThreadId\.value = null;/,
    );
  });

  it('every spawn-then-focus navigation claims the await', () => {
    // Each of these focuses an id the engine has just returned. A plain
    // focusThread lands the user on the compose view, which is what an update
    // of an installed plugin did.
    expect(pluginInstall).toContain('focusSpawnedThread(result.setup_thread_id)');
    expect(pluginInstall).toContain('focusSpawnedThread(result.thread_id)');
    expect(storeTab).toContain('focusSpawnedThread(action.threadId)');
    // And none of them regressed to the bare helper.
    expect(pluginInstall).not.toMatch(/[^a-zA-Z]focusThread\(/);
    expect(storeTab).not.toMatch(/[^a-zA-Z]focusThread\(/);
  });
});
