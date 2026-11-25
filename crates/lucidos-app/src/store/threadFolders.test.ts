import { describe, expect, it } from 'vitest';
import { knownFolderOf, parseThreadFolders, serializeThreadFolders, splitByCustomFolder, type ThreadFolder } from './threadFolders';
import { makeThreadState } from './actions/threads-test-helpers';
import { computeFamilyGraph } from '../components/drawer/family-graph';
import type { ThreadState } from './thread-events';

const W48: ThreadFolder = { id: 'w48', name: 'W48' };
const PROJECT: ThreadFolder = { id: 'project-x', name: 'Project X' };

function pinned(id: string, pinFolder: string | null, parentThreadId?: string): ThreadState {
  return makeThreadState(id, { meta: { saved: true, section: 'inbox', pinFolder, parentThreadId } });
}

function split(threads: ThreadState[], folders: ThreadFolder[]) {
  const graph = computeFamilyGraph(threads);
  const out = splitByCustomFolder(threads, graph.rootByThread, graph.byId, folders);
  return {
    folders: out.folders.map(f => [f.folder.id, f.threads.map(t => t.meta.id)]),
    pinned: out.pinned.map(t => t.meta.id),
  };
}

describe('parseThreadFolders', () => {
  it('reads an unset or unreadable preference as no folders', () => {
    expect(parseThreadFolders(undefined)).toEqual([]);
    expect(parseThreadFolders('')).toEqual([]);
    expect(parseThreadFolders('{not json')).toEqual([]);
    expect(parseThreadFolders('{"id":"a"}')).toEqual([]);
  });

  it('keeps the list order and round-trips', () => {
    const raw = serializeThreadFolders([W48, PROJECT]);
    expect(parseThreadFolders(raw)).toEqual([W48, PROJECT]);
  });
});

describe('splitByCustomFolder', () => {
  it('leaves every pin in Pinned when there are no folders', () => {
    expect(split([pinned('a', null), pinned('b', 'w48')], [])).toEqual({ folders: [], pinned: ['a', 'b'] });
  });

  it('lists a pin whose folder was deleted in Pinned', () => {
    expect(split([pinned('a', 'gone')], [W48])).toEqual({ folders: [['w48', []]], pinned: ['a'] });
  });

  it('keeps an empty folder, in the preference order', () => {
    expect(split([pinned('a', 'w48')], [PROJECT, W48]).folders).toEqual([['project-x', []], ['w48', ['a']]]);
  });

  it('lists a family in its root folder, never split', () => {
    const threads = [pinned('root', 'w48'), pinned('child', null, 'root')];
    expect(split(threads, [W48])).toEqual({ folders: [['w48', ['root', 'child']]], pinned: [] });
  });

  it('keeps the incoming order inside a folder', () => {
    const threads = [pinned('b', 'w48'), pinned('a', 'w48')];
    expect(split(threads, [W48]).folders).toEqual([['w48', ['b', 'a']]]);
  });
});

describe('knownFolderOf', () => {
  it('names the folder only for a pinned thread in a listed folder', () => {
    expect(knownFolderOf(pinned('a', 'w48'), [W48])).toEqual(W48);
    expect(knownFolderOf(pinned('a', 'gone'), [W48])).toBeNull();
    expect(knownFolderOf(makeThreadState('a', { meta: { pinFolder: 'w48' } }), [W48])).toBeNull();
  });
});
