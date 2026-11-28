import { describe, expect, it, vi } from 'vitest';
import {
  CONNECTING_QUIET_MS,
  CONNECTION_BANNER_HEIGHT_VAR,
  connectionBannerBody,
  shouldRenderConnectionBanner,
} from '../ConnectionBanner';
import { BANNER_HEIGHT_VAR } from '../BackupReminderBanner';
import { CONNECTION_POLL_INTERVAL_MS } from '../../../store/store';
import { connectionNotice } from '../../../utils/connectionNotice';
import { findByClass, findByType, textOf } from './vnodeWalk';
import type { BannerLayout } from '../appBanner';
import type { ConnectionStatus } from '../../../store/types';

const DESKTOP = { layout: 'desktop' as BannerLayout, mobileViewport: false };

describe('the bar speaks for exactly the states the mark recedes in', () => {
  it('says nothing at all while connected', () => {
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'connected', connectingSettled: true,
    })).toBe(false);
  });

  it('shows a settled disconnect immediately', () => {
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'disconnected', connectingSettled: false,
    })).toBe(true);
  });

  it('waits out an ordinary load before mentioning connecting', () => {
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'connecting', connectingSettled: false,
    })).toBe(false);
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'connecting', connectingSettled: true,
    })).toBe(true);
  });

  it('waits longer than one poll interval, so a poll has actually been tried', () => {
    expect(CONNECTING_QUIET_MS).toBeGreaterThan(CONNECTION_POLL_INTERVAL_MS);
  });
});

describe('one instance renders, whichever layout is mounted', () => {
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
    expect(CONNECTION_BANNER_HEIGHT_VAR).not.toBe(BANNER_HEIGHT_VAR);
  });
});

describe('connectionBannerBody renders the bar', () => {
  const body = (status: ConnectionStatus, ws: string | null = 'dev') =>
    connectionBannerBody({ layout: 'desktop', status, workspace: ws });

  it('states the notice, from the table every other surface reads', () => {
    const notice = connectionNotice('disconnected', 'dev', 'full')!;
    const text = textOf(body('disconnected'));
    expect(text).toContain(notice.title);
    expect(text).toContain(notice.detail);
  });

  it('is the surface that carries the FULL detail', () => {
    const text = textOf(body('disconnected'));
    const short = connectionNotice('disconnected', 'dev', 'short')!.detail;
    expect(text).toContain(connectionNotice('disconnected', 'dev', 'full')!.detail);
    expect(text.length).toBeGreaterThan(short.length);
  });

  it('leads with a dot in the state colour, and lets the words say it out loud', () => {
    const dots = findByClass(body('disconnected'), 'status-dot');
    expect(dots).toHaveLength(1);
    expect((dots[0].props.class as string).split(' ')).toContain('disconnected');
    expect(dots[0].props['aria-hidden']).toBe('true');
  });

  it('keys its wash on the state, so the two degraded states are told apart', () => {
    for (const status of ['disconnected', 'connecting', 'stopped', 'starting'] as ConnectionStatus[]) {
      const bar = findByClass(body(status), 'connection-banner')[0];
      expect(bar.props['data-conn']).toBe(status);
    }
  });

  it('is a statement: nothing to press, nothing to dismiss', () => {
    const bar = findByClass(body('disconnected'), 'connection-banner')[0];
    expect(bar.props.role).toBe('status');
    expect(bar.props.onClick).toBeUndefined();
    expect(findByType(body('disconnected'), 'button')).toHaveLength(0);
  });

  it('shows a stopped workspace at once, with the same states it recedes in', () => {
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'stopped', connectingSettled: false,
    })).toBe(true);
  });

  it('offers Start while stopped, and only then', () => {
    const onStart = vi.fn();
    const stopped = connectionBannerBody({ layout: 'desktop', status: 'stopped', workspace: 'dev', onStart });
    const buttons = findByType(stopped, 'button');
    expect(buttons).toHaveLength(1);
    expect(textOf(buttons[0])).toBe('Start');
    (buttons[0].props.onClick as () => void)();
    expect(onStart).toHaveBeenCalledOnce();

    for (const status of ['disconnected', 'starting'] as ConnectionStatus[]) {
      const bar = connectionBannerBody({ layout: 'desktop', status, workspace: 'dev', onStart });
      expect(findByType(bar, 'button')).toHaveLength(0);
    }
  });

  it('stays up through a Start, at once, saying the engine is starting', () => {
    expect(shouldRenderConnectionBanner({
      ...DESKTOP, status: 'starting', connectingSettled: false,
    })).toBe(true);
    expect(textOf(body('starting'))).toContain('The dev engine is starting');
  });

  it('draws no Start without a handler, as in a window with no gateway', () => {
    expect(findByType(body('stopped'), 'button')).toHaveLength(0);
  });

  it('disables the button while a Start is in flight, so it cannot double-fire', () => {
    const starting = connectionBannerBody({
      layout: 'desktop', status: 'stopped', workspace: 'dev', onStart: vi.fn(), starting: true,
    });
    const [button] = findByType(starting, 'button');
    expect(button.props.disabled).toBe(true);
    expect(textOf(button)).toBe('Starting…');
  });

  it('carries no left-accent stripe hook', () => {
    const bar = findByClass(body('disconnected'), 'connection-banner')[0];
    expect((bar.props.class as string)).not.toContain('accent-edge');
    expect(bar.props.style).toBeUndefined();
  });

  it('still says something before the workspace has a name', () => {
    expect(textOf(body('connecting', null))).toContain('Connecting');
  });

  it('renders nothing for a state the table does not speak for', () => {
    expect(body('connected')).toBeNull();
  });
});
