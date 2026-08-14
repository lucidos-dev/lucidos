/**
 * The Lucidos menu's connection notice: the words the panel leads with while
 * the mark is dim.
 *
 * The notice is present for exactly the states the mark RECEDES in and absent
 * for the one it does not, so a dimmed glyph can never sit above a panel that
 * mentions nothing. The other half of that pairing is asserted from the
 * stylesheet, in `styles/__tests__/header-mark-geometry.test.ts`: connected is
 * the one state carrying neither an opacity nor an animation, which is what
 * makes "not connected" and "dim" the same set.
 *
 * The wording itself, and the fact that it exists once, is
 * `utils/connectionNotice.test.ts`. This file is the ROW: which element it
 * becomes, which classes it carries, and that it promises no tap.
 *
 * No jsdom: the row is pure, so `vnodeToText` flattens it, and what can be
 * checked that way happens to be exactly what "a statement, not a menu item"
 * means. Same harness as `workspace-switcher.test.tsx`.
 */
import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { connectionNoticeRow } from '../HeaderMark';
import { connectionNotice } from '../../../utils/connectionNotice';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';
import type { ConnectionStatus } from '../../../store/types';

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
    // Read out of the table rather than retyped, which is what
    // `utils/connectionNotice.test.ts` scans for.
    const notice = connectionNotice('disconnected', 'dev', 'short')!;
    expect(text).toContain(notice.title);
    expect(text).toContain(notice.detail);
  });

  it('takes the short detail, leaving the explainer to the bar', () => {
    // The panel is a fixed width, so the full sentence wrapped to three lines
    // and pushed every row below it down. What it bought was a consequence the
    // connection bar states already, on screen and without a tap.
    for (const state of ['disconnected', 'connecting'] as ConnectionStatus[]) {
      const text = row(state);
      expect(text).toContain(connectionNotice(state, 'dev', 'short')!.detail);
      expect(text, `${state} wraps the panel with the bar's explainer`)
        .not.toContain(connectionNotice(state, 'dev', 'full')!.detail);
    }
  });

  it('states the connecting case in its own words, not the settled one', () => {
    const text = row('connecting');
    expect(text).toContain('<span class="status-dot connecting">');
    expect(text).toContain('brand-menu-notice-connecting');
    expect(text).toContain(connectionNotice('connecting', 'dev', 'short')!.title);
    expect(text).not.toContain(connectionNotice('disconnected', 'dev', 'short')!.detail);
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
