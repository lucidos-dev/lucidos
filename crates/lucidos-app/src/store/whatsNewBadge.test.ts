/**
 * The *What's New badge*: what raises it, what it says, and what it refuses to
 * read.
 *
 * The two things it must never do are the interesting half. It must not badge
 * on an unknown answer, which would flash a dot on every cold load. And it must
 * not clear on a DISMISSAL, which would spend the one time the workspace is
 * told.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { whatsNewBadge, whatsNewBadgeLabel } from './whatsNewBadge';
import { releaseNoticeDismissed } from './releaseNotices';
import { latestTauriAppVersion, releaseCheck, releaseNoticeView } from './store';
import type { ReleaseNotice } from '../api/client';

function notice(id: string, resolved: boolean): ReleaseNotice {
  return { id, since: '2.0.0', title: `Notice ${id}`, body: 'Do the thing.', resolved };
}

/** Put `notices` in front of the badge, with the first unresolved one owed. */
function owe(notices: ReleaseNotice[]): void {
  const next = notices.find((n) => !n.resolved)?.id ?? null;
  releaseNoticeView.value = { status: 'loaded', data: { notices, next_id: next } };
}

/** An offer from the gateway, the ordinary way an update is announced. */
function offer(version: string): void {
  releaseCheck.value = {
    enabled: true,
    notice_acknowledged: true,
    supported: true,
    current_version: '0.30.2',
    checked_at: null,
    last_error: null,
    latest: { version, notes: null, install: 'desktop-app', command: null },
  };
}

beforeEach(() => {
  releaseCheck.value = null;
  latestTauriAppVersion.value = null;
  releaseNoticeView.value = { status: 'not-loaded' };
  releaseNoticeDismissed.value = false;
});

describe('whatsNewBadgeLabel', () => {
  it('says nothing when there is nothing to act on', () => {
    expect(whatsNewBadgeLabel(null, 0)).toBe(null);
  });

  it('names the release when one is offered', () => {
    expect(whatsNewBadgeLabel('0.31.0', 0)).toBe('Lucidos 0.31.0 available');
  });

  it('counts what is owed, in the singular and the plural', () => {
    expect(whatsNewBadgeLabel(null, 1)).toBe('1 thing to do');
    expect(whatsNewBadgeLabel(null, 3)).toBe('3 things to do');
  });

  it('states both when both are true', () => {
    expect(whatsNewBadgeLabel('0.31.0', 2)).toBe('Lucidos 0.31.0 available · 2 things to do');
  });
});

describe('whatsNewBadge', () => {
  it('is silent on a quiet workspace', () => {
    owe([notice('a', true)]);
    expect(whatsNewBadge()).toBe(null);
  });

  it('answers the gateway offer', () => {
    offer('0.31.0');
    expect(whatsNewBadge()).toBe('Lucidos 0.31.0 available');
  });

  it('answers an owed notice', () => {
    owe([notice('a', true), notice('b', false)]);
    expect(whatsNewBadge()).toBe('1 thing to do');
  });

  // An unknown answer is not "you owe something". Badging here would draw a dot
  // on every cold load and clear it a moment later.
  it('badges nothing while the notices are unknown', () => {
    releaseNoticeView.value = { status: 'loading' };
    expect(whatsNewBadge()).toBe(null);
    releaseNoticeView.value = { status: 'failed', error: 'engine unreachable' };
    expect(whatsNewBadge()).toBe(null);
  });

  // Escape on the modal answers nothing, so the badge must survive it. The one
  // thing that clears it is the notice being resolved.
  it('stays up when the modal is dismissed unanswered', () => {
    owe([notice('a', false)]);
    releaseNoticeDismissed.value = true;
    expect(whatsNewBadge()).toBe('1 thing to do');
    owe([notice('a', true)]);
    expect(whatsNewBadge()).toBe(null);
  });
});
