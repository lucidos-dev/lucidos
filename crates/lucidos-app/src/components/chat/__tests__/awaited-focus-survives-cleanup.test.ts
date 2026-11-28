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
const storeTab = readFileSync(resolve(here, '../../plugins/pluginCard.tsx'), 'utf-8');

describe('an awaited focus survives ThreadView stale-pointer cleanup', () => {
  it('ThreadView exempts an awaited thread from the unfocus cleanup', () => {
    expect(threadView).toMatch(
      /if\s*\(threadsLoaded\.value\s*&&\s*awaitedThreadId\.value\s*!==\s*threadId\)/,
    );
  });

  it('ThreadView reads the exemption from the store, not a local guess', () => {
    expect(threadView).toMatch(/import\s*\{[^}]*\bawaitedThreadId\b[^}]*\}\s*from\s*'\.\.\/\.\.\/store\/store'/s);
  });

  it('the bootstrap sets the flag and focuses before awaiting the metadata', () => {
    const miss = threads.slice(
      threads.indexOf('export async function focusThreadOrBootstrapResult'),
      threads.indexOf('await ensureThreadByIdInMap'),
    );
    expect(miss).toContain('awaitedThreadId.value = threadId');
    expect(miss).toContain('setFocusedThread(threadId)');
    expect(miss).toContain('revealThreadPane()');
  });

  it('the bootstrap retires the standing follow at its OWN focus, not at the focusThread it ends with', () => {
    const miss = threads.slice(
      threads.indexOf('export async function focusThreadOrBootstrapResult'),
      threads.indexOf('await ensureThreadByIdInMap'),
    );
    expect(miss).toContain('stopFollowingBottom()');
  });

  it('every non-focused exit from the bootstrap releases the flag', () => {
    expect(threads).toMatch(/catch \(error\) \{\s*releaseAwait\(threadId, previousFocus\);/);
    expect(threads).toMatch(/if \(await threadQueueHolds\(threadId\)\) return \{ kind: 'awaiting' \};/);
    expect(threads).toMatch(/if \(!found\) \{\s*releaseAwait\(threadId, previousFocus\);/);
    expect(threads).toMatch(/if \(awaitedThreadId\.value === threadId\) awaitedThreadId\.value = null;/);
  });

  it('ThreadView releases the await once the thread lands in the map', () => {
    expect(threadView).toMatch(
      /threadInMap\s*&&\s*awaitedThreadId\.value === threadId\)\s*\{\s*awaitedThreadId\.value = null;/,
    );
  });

  it('every spawn-then-focus navigation claims the await', () => {
    expect(pluginInstall).toContain('focusSpawnedThread(result.setup_thread_id)');
    expect(pluginInstall).toContain('focusSpawnedThread(result.thread_id)');
    expect(storeTab).toContain('focusSpawnedThread(action.threadId)');
    expect(pluginInstall).not.toMatch(/[^a-zA-Z]focusThread\(/);
    expect(storeTab).not.toMatch(/[^a-zA-Z]focusThread\(/);
  });
});
