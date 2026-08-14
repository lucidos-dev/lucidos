/**
 * The header's connection bar: the words for a bad connection, on screen rather
 * than behind a tap.
 *
 * Three properties. It renders for exactly the states the mark recedes in, which
 * is the pairing the whole notice family is built on. It renders ONCE per
 * viewport, the dual-render rule every app-shell banner obeys. And it is a
 * statement: no button, no dismissal, nothing that promises a tap, because the
 * sentence is the whole message and neither remedy in the Lucidos menu can fix a
 * disconnect.
 *
 * Components are invoked as plain functions and the returned vnode tree is
 * walked (the repo idiom, no DOM render library), which is why the markup lives
 * in the hook-free `connectionBannerBody` and the gate in
 * `shouldRenderConnectionBanner`.
 */
import { describe, expect, it } from 'vitest';
import {
  CONNECTING_QUIET_MS,
  CONNECTION_BANNER_HEIGHT_VAR,
  connectionBannerBody,
  shouldRenderConnectionBanner,
} from '../ConnectionBanner';
import { BANNER_HEIGHT_VAR } from '../BackupReminderBanner';
import { connectionNotice } from '../../../utils/connectionNotice';
import { findByClass, findByType, textOf } from './vnodeWalk';
import type { BannerLayout } from '../appBanner';
import type { ConnectionStatus } from '../../../store/types';

const DESKTOP = { layout: 'desktop' as BannerLayout, mobileViewport: false };

describe('the bar speaks for exactly the states the mark recedes in', () => {
  it('says nothing at all while connected', () => {
    // The mark is at full light, so a bar would be restating a state nobody
    // needs told. Same condition as the menu notice, from the same table.
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'connected', connectingSettled: true,
    })).toBe(false);
  });

  it('shows a settled disconnect immediately', () => {
    // `disconnected` already costs MAX_SUPPRESSED_FAILURES + 1 failed polls
    // (roughly 20s) to reach, so a second fuse here would only make the user
    // wait twice for one piece of news.
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'disconnected', connectingSettled: false,
    })).toBe(true);
  });

  it('waits out an ordinary load before mentioning connecting', () => {
    // `connecting` is the state before the FIRST health answer and normally
    // resolves in one tick: announcing it at once would put a bar on screen
    // during every cold start.
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'connecting', connectingSettled: false,
    })).toBe(false);
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'connecting', connectingSettled: true,
    })).toBe(true);
  });

  it('waits longer than one poll interval, so a poll has actually been tried', () => {
    // The health poll runs every 5s (store/actions/connection.ts). A fuse
    // shorter than that fires while the first probe is still in flight, which
    // is not yet news.
    expect(CONNECTING_QUIET_MS).toBeGreaterThan(5000);
  });
});

describe('one instance renders, whichever layout is mounted', () => {
  // Both are mounted (the mobile one inside the fixed header, the desktop one in
  // the shell's flow). Rendering both would show two bars and race two
  // ResizeObservers.
  const state = { status: 'disconnected' as ConnectionStatus, connectingSettled: true };

  it('renders only the desktop instance on a desktop viewport', () => {
    expect(shouldRenderConnectionBanner({ layout: 'desktop', mobileViewport: false, ...state })).toBe(true);
    expect(shouldRenderConnectionBanner({ layout: 'mobile', mobileViewport: false, ...state })).toBe(false);
  });

  it('renders only the mobile instance on a mobile viewport', () => {
    expect(shouldRenderConnectionBanner({ layout: 'mobile', mobileViewport: true, ...state })).toBe(true);
    expect(shouldRenderConnectionBanner({ layout: 'desktop', mobileViewport: true, ...state })).toBe(false);
  });
});

describe('the two banners never share a height reservation', () => {
  it('each publishes its own property', () => {
    // Both bars can be up at once, and each measures itself: one shared property
    // would mean whichever measured last wins, and retracting either would clear
    // the space the other still occupies.
    expect(CONNECTION_BANNER_HEIGHT_VAR).not.toBe(BANNER_HEIGHT_VAR);
  });
});

describe('connectionBannerBody renders the bar', () => {
  const body = (status: ConnectionStatus, ws: string | null = 'dev') =>
    connectionBannerBody({ layout: 'desktop', status, workspace: ws });

  it('states the notice, from the table every other surface reads', () => {
    const notice = connectionNotice('disconnected', 'dev')!;
    const text = textOf(body('disconnected'));
    expect(text).toContain(notice.title);
    expect(text).toContain(notice.detail);
  });

  it('leads with a dot in the state colour, and lets the words say it out loud', () => {
    const dots = findByClass(body('disconnected'), 'status-dot');
    expect(dots).toHaveLength(1);
    // The colour comes from the shared `.status-dot` scale, so the state word
    // has to reach the class list for it to be anything but muted.
    expect((dots[0].props.class as string).split(' ')).toContain('disconnected');
    // Decorative: the sentence beside it already says the state, and announcing
    // both reads it out twice.
    expect(dots[0].props['aria-hidden']).toBe('true');
  });

  it('keys its wash on the state, so the two degraded states are told apart', () => {
    for (const status of ['disconnected', 'connecting'] as ConnectionStatus[]) {
      const bar = findByClass(body(status), 'connection-banner')[0];
      expect(bar.props['data-conn']).toBe(status);
    }
  });

  it('is a statement: nothing to press, nothing to dismiss', () => {
    // Unlike the backup reminder, which stays true until the user acts, this
    // retracts itself on the next good poll. A dismiss control would only offer
    // a way to hide a live fault.
    const bar = findByClass(body('disconnected'), 'connection-banner')[0];
    expect(bar.props.role).toBe('status');
    expect(bar.props.onClick).toBeUndefined();
    expect(findByType(body('disconnected'), 'button')).toHaveLength(0);
  });

  it('carries no left-accent stripe hook', () => {
    // Banned outright by .claude/rules/frontend-css.md; the wash is the emphasis.
    const bar = findByClass(body('disconnected'), 'connection-banner')[0];
    expect((bar.props.class as string)).not.toContain('accent-edge');
    expect(bar.props.style).toBeUndefined();
  });

  it('still says something before the workspace has a name', () => {
    // The window before /health answers is exactly when this is most likely to
    // be read, and it is the window with no name to put in the sentence.
    expect(textOf(body('connecting', null))).toContain('Connecting');
  });

  it('renders nothing for a state the table does not speak for', () => {
    expect(body('connected')).toBeNull();
  });
});
