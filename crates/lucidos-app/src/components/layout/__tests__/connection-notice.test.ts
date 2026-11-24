import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { connectionNoticeRow } from '../BrandMenuRows';
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
    expect(text).toContain('<span class="status-dot disconnected">');
    expect(text).toContain('brand-menu-notice-disconnected');
    const notice = connectionNotice('disconnected', 'dev', 'short')!;
    expect(text).toContain(notice.title);
    expect(text).toContain(notice.detail);
  });

  it('takes the short detail, leaving the explainer to the bar', () => {
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
    const vnode = connectionNoticeRow('disconnected', 'dev') as VNode<Record<string, unknown>>;
    expect(vnode.type).toBe('div');
    expect(vnode.props.role).toBe('none');
    expect(vnode.props.onClick).toBeUndefined();
    const text = row('disconnected');
    expect(text).not.toContain('brand-menu-item');
    expect(text).not.toContain('<button');
  });
});
