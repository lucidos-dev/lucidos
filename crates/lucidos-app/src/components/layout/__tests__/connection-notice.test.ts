/**
 * The Lucidos menu's connection notice: the words the panel leads with while
 * the mark is dim.
 *
 * Two properties, and the first is the whole point of the surface. The notice
 * is present for exactly the states the mark RECEDES in and absent for the one
 * it does not, so a dimmed glyph can never sit above a panel that mentions
 * nothing. The other half of that pairing is asserted from the stylesheet, in
 * `styles/__tests__/header-mark-geometry.test.ts`: connected is the one state
 * carrying neither an opacity nor an animation, which is what makes "not
 * connected" and "dim" the same set.
 *
 * The second is that the sentence comes from the SAME table the toggle's
 * accessible name and the desktop tooltip read (`connectionPhrase`), so the two
 * surfaces cannot end up making different claims about one state.
 *
 * No jsdom: the row is pure, so `vnodeToText` flattens it, and what can be
 * checked that way (which element it becomes, which classes it carries) happens
 * to be exactly what "a statement, not a menu item" means. Same harness as
 * `workspace-switcher.test.tsx`.
 */
import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { connectionNotice, connectionNoticeRow, connectionPhrase } from '../HeaderMark';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';
import type { ConnectionStatus } from '../../../store/types';

const DIMMED: ConnectionStatus[] = ['disconnected', 'connecting'];

describe('connectionNotice', () => {
  it('speaks for exactly the states the mark dims for', () => {
    for (const state of DIMMED) {
      expect(connectionNotice(state, 'dev'), `${state} leaves the mark receded`).not.toBeNull();
    }
    // The mark is at full strength, so the panel has nothing to explain.
    expect(connectionNotice('connected', 'dev')).toBeNull();
  });

  it('titles itself with the phrase the toggle already says, sentence-cased', () => {
    // One table, so the tooltip cannot say "disconnected from dev" while the
    // panel says something else. The preposition each state wants is decided in
    // `connectionPhrase` and nowhere twice.
    for (const state of DIMMED) {
      const phrase = connectionPhrase(state, 'dev');
      expect(connectionNotice(state, 'dev')!.title)
        .toBe(phrase.charAt(0).toUpperCase() + phrase.slice(1));
    }
    expect(connectionNotice('disconnected', 'dev')!.title).toBe('Disconnected from dev');
    expect(connectionNotice('connecting', 'dev')!.title).toBe('Connecting to dev');
  });

  it('falls back to the bare state before the workspace has a name', () => {
    // The window before /health answers, which is exactly when the mark is
    // breathing and the notice is most likely to be read.
    expect(connectionNotice('connecting', null)!.title).toBe('Connecting');
    expect(connectionNotice('disconnected', '')!.title).toBe('Disconnected');
  });

  it('promises recovery only where recovery is honest', () => {
    // Neither row below the notice can fix a disconnect: Restart posts to the
    // engine we cannot reach, and Refresh reloads a client that is not what
    // broke. The health poll genuinely does recover on its own, so that is the
    // only thing the line may claim.
    const detail = connectionNotice('disconnected', 'dev')!.detail;
    expect(detail).toContain('Still trying');
    for (const remedy of ['Refresh', 'Restart']) {
      expect(detail, `naming ${remedy} as the fix would be wrong in the ordinary case`)
        .not.toContain(remedy);
    }
  });

  it('claims only this workspace, since the gateway is a different process', () => {
    // `connectionStatus` is driven solely by `/api/v1/health` against this
    // workspace's engine, and the Workspaces row under the notice reaches the
    // GATEWAY instead, so it keeps listing and switching through an engine
    // outage. A blanket "nothing loads or sends" is refuted by the row directly
    // below the sentence making the claim.
    expect(connectionNotice('disconnected', 'dev')!.detail).toContain('in this workspace');
  });
});

describe('the notice as rendered', () => {
  const row = (state: ConnectionStatus, ws: string | null = 'dev') =>
    vnodeToText(connectionNoticeRow(state, ws));

  it('renders nothing at all while the mark is lit', () => {
    expect(connectionNoticeRow('connected', 'dev')).toBeNull();
    expect(row('connected')).toBe('');
  });

  it('leads with a dot in the state colour and says the state in words', () => {
    const text = row('disconnected');
    // The dot's colour comes from the shared `.status-dot` scale, so the state
    // word has to reach the class list for it to be anything but muted.
    expect(text).toContain('<span class="status-dot disconnected">');
    expect(text).toContain('brand-menu-notice-disconnected');
    expect(text).toContain('Disconnected from dev');
    expect(text).toContain('Nothing in this workspace loads or sends');
  });

  it('states the connecting case in its own words, not the settled one', () => {
    const text = row('connecting');
    expect(text).toContain('<span class="status-dot connecting">');
    expect(text).toContain('brand-menu-notice-connecting');
    expect(text).toContain('Connecting to dev');
    expect(text).not.toContain('Still trying');
  });

  it('is a statement, not a menu item', () => {
    // Three things at once. It is a <div>, so nothing about it invites a tap;
    // it wears none of the row classes, so it takes no hover background and no
    // pointer cursor; and it is `role="none"`, because the panel is a
    // `role="menu"` and an announced node among its children is an orphan the
    // keyboard roving would have to step past. The state is already in the
    // accessible name of the control that opened the panel.
    const vnode = connectionNoticeRow('disconnected', 'dev') as VNode<Record<string, unknown>>;
    expect(vnode.type).toBe('div');
    expect(vnode.props.role).toBe('none');
    expect(vnode.props.onClick).toBeUndefined();
    const text = row('disconnected');
    expect(text).not.toContain('brand-menu-item');
    expect(text).not.toContain('<button');
  });
});
