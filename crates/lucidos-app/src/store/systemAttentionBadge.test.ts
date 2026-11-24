import { describe, it, expect, beforeEach, vi } from 'vitest';

const platform = vi.hoisted(() => ({ thisDeviceIsMobile: vi.fn(() => false) }));
vi.mock('../utils/platform', () => platform);

import { systemAttentionBadge, systemAttentionBadgeLabel } from './systemAttentionBadge';
import { releaseNoticeDismissed } from './releaseNotices';
import { latestTauriAppVersion, releaseCheck, releaseNoticeView } from './store';
import type { ReleaseNoticeItem } from '../api/client';

function notice(id: string, resolved: boolean): ReleaseNoticeItem {
  return { id, since: '2.0.0', title: `Notice ${id}`, body: 'Do the thing.', summary: 'Review it.', resolved };
}

function owe(items: ReleaseNoticeItem[]): void {
  const owed = items.filter((item) => !item.resolved);
  const owedThrough = owed[owed.length - 1]?.id ?? null;
  releaseNoticeView.value = {
    status: 'loaded',
    data: {
      releases: [{ since: '2.0.0', items, resolved: owedThrough === null, action_prompt: null }],
      owed_through: owedThrough,
      action_prompt: null,
    },
  };
}

function offer(version: string): void {
  releaseCheck.value = {
    enabled: true,
    supported: true,
    current_version: '0.30.2',
    checked_at: null,
    last_error: null,
    latest: { version, notes: null, install: 'desktop-app', command: null },
    update_address: null,
    update_address_error: null,
  };
}

beforeEach(() => {
  releaseCheck.value = null;
  latestTauriAppVersion.value = null;
  releaseNoticeView.value = { status: 'not-loaded' };
  releaseNoticeDismissed.value = false;
  platform.thisDeviceIsMobile.mockReturnValue(false);
});

describe('systemAttentionBadgeLabel', () => {
  it('says nothing when there is nothing to act on', () => {
    expect(systemAttentionBadgeLabel(null, 0)).toBe(null);
  });

  it('names the release when one is offered', () => {
    expect(systemAttentionBadgeLabel('9.9.9', 0)).toBe('Lucidos 9.9.9 available');
  });

  it('counts what is owed, in the singular and the plural', () => {
    expect(systemAttentionBadgeLabel(null, 1)).toBe('1 thing to do');
    expect(systemAttentionBadgeLabel(null, 3)).toBe('3 things to do');
  });

  it('states both when both are true', () => {
    expect(systemAttentionBadgeLabel('9.9.9', 2)).toBe('Lucidos 9.9.9 available · 2 things to do');
  });
});

describe('systemAttentionBadge', () => {
  it('is silent on a quiet workspace', () => {
    owe([notice('a', true)]);
    expect(systemAttentionBadge()).toBe(null);
  });

  it('answers the gateway offer', () => {
    offer('9.9.9');
    expect(systemAttentionBadge()).toBe('Lucidos 9.9.9 available');
  });

  it('answers an owed notice', () => {
    owe([notice('a', true), notice('b', false), notice('c', false)]);
    expect(systemAttentionBadge()).toBe('1 thing to do');
  });

  it('badges nothing while the notices are unknown', () => {
    releaseNoticeView.value = { status: 'loading' };
    expect(systemAttentionBadge()).toBe(null);
    releaseNoticeView.value = { status: 'failed', error: 'engine unreachable' };
    expect(systemAttentionBadge()).toBe(null);
  });

  it('does not raise the update half on a phone', () => {
    platform.thisDeviceIsMobile.mockReturnValue(true);
    offer('9.9.9');
    expect(systemAttentionBadge()).toBe(null);
  });

  it('still raises an owed notice on a phone', () => {
    platform.thisDeviceIsMobile.mockReturnValue(true);
    offer('9.9.9');
    owe([notice('a', false)]);
    expect(systemAttentionBadge()).toBe('1 thing to do');
  });

  it('stays up when the modal is dismissed unanswered', () => {
    owe([notice('a', false)]);
    releaseNoticeDismissed.value = true;
    expect(systemAttentionBadge()).toBe('1 thing to do');
    owe([notice('a', true)]);
    expect(systemAttentionBadge()).toBe(null);
  });
});
