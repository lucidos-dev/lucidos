/** A new-chat navigate's `coding_agent` and `folder` picks, against the real
 *  compose store: they land on the NEW draft only, and a value that names
 *  nothing leaves the dropdown at its default and toasts. */

import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') {
    (globalThis as any).document = {};
  }
  if (!(globalThis.document as any).querySelector) {
    (globalThis.document as any).querySelector = () => null;
  }
  if (!(globalThis.document as any).querySelectorAll) {
    (globalThis.document as any).querySelectorAll = () => [];
  }
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

const { showToast } = vi.hoisted(() => ({ showToast: vi.fn() }));
vi.mock('../store', async (importActual) => ({ ...(await importActual<typeof import('../store')>()), showToast }));

const { submitChat } = vi.hoisted(() => ({ submitChat: vi.fn() }));
vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  API_BASE: '',
  submitChat,
  putComposeOnThread: vi.fn().mockResolvedValue({ status: 'applied' }),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./thread-navigation', () => ({ pushThreadNavState: vi.fn(), removeThreadNavEntries: vi.fn() }));
vi.mock('./devices', () => ({ getDeviceId: () => 'device-test' }));
vi.mock('../../utils/platform', () => ({ isTauri: () => false, isIOS: () => false }));

const { refreshRepositories } = vi.hoisted(() => ({ refreshRepositories: vi.fn(async () => {}) }));
vi.mock('./repositoriesLoader', () => ({ refreshRepositories }));
const { refreshApps } = vi.hoisted(() => ({ refreshApps: vi.fn(async () => {}) }));
vi.mock('./apps', () => ({ refreshApps }));

import {
  appsList,
  focusedThreadId,
  inputMode,
  repositories,
  selectedCodingAgent,
  selectedScope,
  threadMap,
  type Repository,
} from '../store';
import type { App } from '../types';
import { LUCIDOS_SOURCE_REPO_NAME } from '../composeDestination';
import { getComposeSelectionOverride, resolveCodingAgent, resolveScope, seedComposeSelection, _resetComposeSelectionsForTesting } from '../composeSelections';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { makeOptimisticThreadState } from '../thread-events';
import { prefillNewChatDestination } from './newChatPrefill';

const DRAFT = 'new-draft';
const OTHER = 'other-draft';
const SOURCE = 'thread "My PRs"';

const repo = (id: string, name: string): Repository => ({ id, name, path: `/repos/${name}` });
const EXAMPLE = repo('repo-1', 'example-repo');

function putDraft(id: string): void {
  const next = new Map(threadMap.value);
  next.set(id, makeOptimisticThreadState({
    id, title: '', channel: 'chat', initiator: 'user', eventsLoaded: true, state: 'composing', status: 'idle',
  }));
  threadMap.value = next;
  setDraft(id, { text: 'Work on PR #123', image_hashes: [], mode: 'lucidos' });
}

beforeEach(() => {
  vi.clearAllMocks();
  threadMap.value = new Map();
  inputMode.value = { type: 'do' };
  selectedScope.value = { kind: 'lucidos' };
  selectedCodingAgent.value = 'claude-code';
  repositories.value = { status: 'loaded', data: [EXAMPLE, repo('repo-2', 'other-repo')] };
  appsList.value = { status: 'loaded', data: [{ id: 'habit-tracker', name: 'Habit Tracker' } as App] };
  _resetComposeDraftsForTesting();
  _resetComposeSelectionsForTesting();
  putDraft(DRAFT);
  putDraft(OTHER);
  seedComposeSelection(OTHER, { scope: { kind: 'external', repoId: 'repo-2' }, codingAgent: 'claude-code' });
  focusedThreadId.value = DRAFT;
});

function expectOtherDraftUntouched(): void {
  expect(getComposeSelectionOverride(OTHER)).toEqual({ scope: { kind: 'external', repoId: 'repo-2' }, codingAgent: 'claude-code' });
  expect(getDraft(OTHER).mode).toBe('lucidos');
  expect(selectedCodingAgent.value).toBe('claude-code');
  expect(submitChat).not.toHaveBeenCalled();
}

describe('prefillNewChatDestination', () => {
  it('writes nothing when neither field is given', () => {
    prefillNewChatDestination(DRAFT, {}, SOURCE);
    prefillNewChatDestination(DRAFT, { coding_agent: '', folder: '' }, SOURCE);
    expect(inputMode.value).toEqual({ type: 'do' });
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expect(getDraft(DRAFT).mode).toBe('lucidos');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('preselects a registered repository by name on the new draft only', () => {
    expect(prefillNewChatDestination(DRAFT, { folder: 'example-repo' }, SOURCE)).toBeUndefined();
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-1' });
    expect(getDraft(DRAFT).mode).toBe('claude_code');
    expect(getDraft(DRAFT).text).toBe('Work on PR #123');
    expect(showToast).not.toHaveBeenCalled();
    expectOtherDraftUntouched();
  });

  it('matches a repository by id and by name in any case', () => {
    prefillNewChatDestination(DRAFT, { folder: 'repo-2' });
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-2' });
    prefillNewChatDestination(DRAFT, { folder: 'EXAMPLE-REPO' });
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-1' });
  });

  it('maps the Lucidos source repository to the Lucidos destination', () => {
    repositories.value = { status: 'loaded', data: [repo('src-id', LUCIDOS_SOURCE_REPO_NAME)] };
    seedComposeSelection(DRAFT, { scope: { kind: 'external', repoId: 'repo-1' } });
    prefillNewChatDestination(DRAFT, { folder: LUCIDOS_SOURCE_REPO_NAME });
    expect(resolveScope(DRAFT)).toEqual({ kind: 'lucidos' });
    expect(getDraft(DRAFT).mode).toBe('claude_code');
  });

  it('preselects an installed app named as data/apps/<id>', () => {
    prefillNewChatDestination(DRAFT, { folder: 'data/apps/habit-tracker' }, SOURCE);
    expect(resolveScope(DRAFT)).toEqual({ kind: 'app', appId: 'habit-tracker' });
    expect(getDraft(DRAFT).mode).toBe('claude_code');
    expectOtherDraftUntouched();
  });

  it('preselects the coding agent and keeps the draft\'s seeded scope', () => {
    seedComposeSelection(DRAFT, { scope: { kind: 'external', repoId: 'repo-1' } });
    prefillNewChatDestination(DRAFT, { coding_agent: 'codex' }, SOURCE);
    expect(resolveCodingAgent(DRAFT)).toBe('codex');
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-1' });
    expect(getDraft(DRAFT).mode).toBe('claude_code');
    expectOtherDraftUntouched();
  });

  it('applies the folder and the agent together', () => {
    prefillNewChatDestination(DRAFT, { coding_agent: 'codex', folder: 'example-repo' }, SOURCE);
    expect(resolveCodingAgent(DRAFT)).toBe('codex');
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-1' });
    expectOtherDraftUntouched();
  });

  it('re-reads the registry for a repository registered moments ago', async () => {
    refreshRepositories.mockImplementationOnce(async () => {
      repositories.value = { status: 'loaded', data: [EXAMPLE, repo('repo-3', 'fresh-repo')] };
    });
    await prefillNewChatDestination(DRAFT, { folder: 'fresh-repo' }, SOURCE);
    expect(refreshRepositories).toHaveBeenCalledTimes(1);
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-3' });
    expect(showToast).not.toHaveBeenCalled();
    expectOtherDraftUntouched();
  });

  it('leaves the default and warns for a folder that names no repository', async () => {
    await prefillNewChatDestination(DRAFT, { folder: 'no-such-repo' }, SOURCE);
    expect(refreshRepositories).toHaveBeenCalledTimes(1);
    expect(inputMode.value).toEqual({ type: 'do' });
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expect(getDraft(DRAFT).mode).toBe('lucidos');
    expect(getDraft(DRAFT).text).toBe('Work on PR #123');
    expect(showToast).toHaveBeenCalledTimes(1);
    const [message, kind] = showToast.mock.calls[0];
    expect(message).toContain('"no-such-repo"');
    expect(message).toContain(`requested by ${SOURCE}`);
    expect(kind).toBe('error');
    expectOtherDraftUntouched();
  });

  it('leaves the default and warns for an ambiguous repository name', async () => {
    repositories.value = { status: 'loaded', data: [repo('a', 'twin'), repo('b', 'twin')] };
    await prefillNewChatDestination(DRAFT, { folder: 'twin' }, SOURCE);
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('2 registered repositories share that name'), 'error');
  });

  it('warns when the registry cannot be read', async () => {
    repositories.value = { status: 'not-loaded' };
    refreshRepositories.mockImplementationOnce(async () => {
      repositories.value = { status: 'failed', error: 'offline' };
    });
    await prefillNewChatDestination(DRAFT, { folder: 'example-repo' }, SOURCE);
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('failed to load repositories: offline'), 'error');
  });

  it('warns for an app that is not installed, after re-reading the apps list', async () => {
    await prefillNewChatDestination(DRAFT, { folder: 'data/apps/ghost' }, SOURCE);
    expect(refreshApps).toHaveBeenCalledTimes(1);
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('no installed app has the id "ghost"'), 'error');
  });

  it('warns for an unknown coding agent but still applies a valid folder', () => {
    prefillNewChatDestination(DRAFT, { coding_agent: 'gemini', folder: 'example-repo' }, SOURCE);
    expect(showToast).toHaveBeenCalledTimes(1);
    const [message] = showToast.mock.calls[0];
    expect(message).toContain('"gemini"');
    expect(message).toContain(`requested by ${SOURCE}`);
    expect(resolveScope(DRAFT)).toEqual({ kind: 'external', repoId: 'repo-1' });
    expect(getComposeSelectionOverride(DRAFT).codingAgent).toBeUndefined();
    expectOtherDraftUntouched();
  });

  it('writes nothing when the only pick is an unknown coding agent', () => {
    prefillNewChatDestination(DRAFT, { coding_agent: 'gemini' }, SOURCE);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(inputMode.value).toEqual({ type: 'do' });
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expect(getDraft(DRAFT).mode).toBe('lucidos');
  });

  it('applies nothing once the reader has moved to another draft during the re-read', async () => {
    refreshRepositories.mockImplementationOnce(async () => {
      repositories.value = { status: 'loaded', data: [EXAMPLE, repo('repo-3', 'fresh-repo')] };
      focusedThreadId.value = OTHER;
    });
    await prefillNewChatDestination(DRAFT, { folder: 'fresh-repo' }, SOURCE);
    expect(inputMode.value).toEqual({ type: 'do' });
    expect(getComposeSelectionOverride(DRAFT)).toEqual({});
    expectOtherDraftUntouched();
  });
});
