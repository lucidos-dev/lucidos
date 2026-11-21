/**
 * The memory choice shows exactly when the workspace has never chosen its
 * memory module, a provider is set up, and no release notice is up. Every
 * answer writes the preference, so a stored choice of either kind hides it.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import type { ReleaseNoticeView } from '../../api/client';
import { memoryChoiceOpen } from '../memoryChoice';
import { releaseNoticeDismissed } from '../releaseNotices';
import { engineStartedAt, llmConfigured, preferences, releaseNoticeView } from '../store';

const NOTHING_OWED: ReleaseNoticeView = { notices: [], next_id: null };
const ONE_OWED: ReleaseNoticeView = {
  notices: [{ id: 'a', since: '2.0.0', title: 'Notice a', body: 'Do the thing.', resolved: false, action_deferred: false }],
  next_id: 'a',
};

describe('memoryChoiceOpen', () => {
  beforeEach(() => {
    preferences.value = { status: 'loaded', data: {} };
    engineStartedAt.value = '2026-10-14T10:00:00Z';
    llmConfigured.value = true;
    releaseNoticeView.value = { status: 'loaded', data: NOTHING_OWED };
    releaseNoticeDismissed.value = false;
  });

  it('shows on a workspace that never chose, with a provider and no notice up', () => {
    expect(memoryChoiceOpen()).toBe(true);
  });

  /** An upgraded workspace stores `classic` at upgrade, so it is never asked. */
  it.each(['classic', 'tree'])('hides once %s is stored', (module) => {
    preferences.value = { status: 'loaded', data: { memory_module: module } };
    expect(memoryChoiceOpen()).toBe(false);
  });

  it('asks again only for a value the catalog refuses, which is no choice', () => {
    preferences.value = { status: 'loaded', data: { memory_module: 'bogus' } };
    expect(memoryChoiceOpen()).toBe(true);
  });

  it('waits for preferences', () => {
    preferences.value = { status: 'loading' };
    expect(memoryChoiceOpen()).toBe(false);
  });

  it('waits for the health probe, then for a provider', () => {
    engineStartedAt.value = null;
    expect(memoryChoiceOpen()).toBe(false);
    engineStartedAt.value = '2026-10-14T10:00:00Z';
    llmConfigured.value = false;
    expect(memoryChoiceOpen()).toBe(false);
  });

  it('waits until the notice list has answered, or failed to', () => {
    for (const status of ['not-loaded', 'loading'] as const) {
      releaseNoticeView.value = { status };
      expect(memoryChoiceOpen(), status).toBe(false);
    }
    releaseNoticeView.value = { status: 'failed', error: 'engine unreachable' };
    expect(memoryChoiceOpen()).toBe(true);
  });

  it('waits while a release notice is up, and shows once it is dismissed', () => {
    releaseNoticeView.value = { status: 'loaded', data: ONE_OWED };
    expect(memoryChoiceOpen()).toBe(false);
    releaseNoticeDismissed.value = true;
    expect(memoryChoiceOpen()).toBe(true);
  });
});
