import { vi, describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { brandBadgeState, brandBadgeLabel, brandTooltipRows, BrandBadge, UnreadBrandBadge, unreadBadgeLabel } from './BrandBadge';
import { GlyphBadge } from '../shared/GlyphBadge';
import { vnodeToText } from '../chat/__tests__/vnodeToText';
import { notificationsMenuUnreadTotal, peerWorkspaces, remotePeerWorkspaces } from '../../store/actions/app-badge';
import type { ActivityRow } from '../../store/actions/activityRows';
import { connectionNotice } from '../../utils/connectionNotice';
import type { Notification } from '../../store/types';
import {
  restartRequired,
  engineVersionReady,
  engineVersionPending,
  engineBuilding,
  engineBuildDetail,
  enginePackaged,
  updateAvailable,
  embeddingModelStatus,
  applyAllInProgress,
  engineRestarting,
  mobileView,
  servedUnreadNotifications,
} from '../../store/store';
vi.mock('../shared/GlyphBadge', () => import('../shared/__tests__/glyphBadgeStub'));

describe('brandBadgeState / brandBadgeLabel', () => {
  const build: ActivityRow = {
    key: 'engine-build',
    label: 'Building new version',
    detail: '2m 14s',
    body: { kind: 'background', activity: { kind: 'engine-build', label: 'Building new version', progress: null } },
  };
  const apply: ActivityRow = {
    key: 'apply-all',
    label: 'Applying 4 changes',
    detail: '0 done',
    body: {
      kind: 'apply-all',
      thread: { threadId: 't-1', changeId: 'c-1', reading: null, title: 'Card gate fix', phase: 'Applying changes' },
      count: { done: 0, total: 4 },
      progress: { done: 0, working: 0.25 },
      timeLeft: null,
      canceling: false,
    },
  };

  beforeEach(() => {
    restartRequired.value = false;
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    engineBuilding.value = false;
    enginePackaged.value = false;
    updateAvailable.value = false;
  });

  it('nothing pending, no badge', () => {
    expect(brandBadgeState(0)).toBe('none');
    expect(brandBadgeLabel([])).toBeUndefined();
  });

  it('dev at Apply time (restart pending, build not ready) shows no engine badge', () => {
    restartRequired.value = true;
    expect(brandBadgeState(0)).toBe('none');
    expect(brandBadgeLabel([])).toBeUndefined();
  });

  it('a job in flight shows the busy badge, named in the menu row\'s words', () => {
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeLabel([build])).toBe('Building new version');
  });

  it('concurrent jobs are named together in one tooltip', () => {
    expect(brandBadgeState(2)).toBe('busy');
    expect(brandBadgeLabel([build, apply])).toBe('Building new version · Applying 4 changes');
  });

  it('busy wins over a concurrently-ready signal (switch not offered until the work lands)', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeLabel([build])).toBe('Building new version');
  });

  it('dev with the rebuild ready shows the attention (!) badge', () => {
    engineVersionReady.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeLabel([])).toBe('New version available');
  });

  it('engine-ready + client update available is still one attention badge', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeLabel([])).toBe('New version available · Client update available');
  });

  it('client update alone (engine idle) shows the attention badge with the client tooltip', () => {
    updateAvailable.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeLabel([])).toBe('Client update available');
  });

  it('source ahead with nothing built shows the pending badge', () => {
    engineVersionPending.value = true;
    expect(brandBadgeState(0)).toBe('pending');
    expect(brandBadgeLabel([])).toBe('New code pending');
  });


  it('ready wins over pending: something you can take now beats something unbuilt', () => {
    engineVersionReady.value = true;
    engineVersionPending.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeLabel([])).toBe('New version available');
  });

  it('busy wins over pending: a build in flight may yet resolve it', () => {
    engineVersionPending.value = true;
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeLabel([build])).toBe('Building new version');
  });
});

describe('brandTooltipRows', () => {
  const conflict = (threadId: string): ActivityRow => ({
    key: `apply-${threadId}`,
    label: `Resolving merge conflict: ${threadId}`,
    body: {
      kind: 'apply-thread',
      thread: {
        threadId,
        changeId: null,
        reading: { phase: 'resolving-conflict', eventId: null, startedAt: null },
        title: threadId,
        phase: 'Resolving merge conflict',
      },
    },
  });
  const batch: ActivityRow = {
    key: 'apply-all',
    label: 'Applying 14 changes',
    detail: '1 done',
    body: { kind: 'apply-all', thread: null, count: { done: 1, total: 14 }, progress: null, timeLeft: null, canceling: false },
  };

  beforeEach(() => {
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    updateAvailable.value = false;
  });

  it('is empty when there is nothing to report, so the name stands alone', () => {
    expect(brandTooltipRows(0, [], null)).toEqual([]);
  });

  it('collapses single-thread applies to one count per phase', () => {
    expect(brandTooltipRows(4, [batch, conflict('a'), conflict('b'), conflict('c')], null)).toEqual([
      { label: 'Unread', value: '4' },
      { label: 'Applying 14 changes', value: '1 done' },
      { label: 'Merge conflicts', value: '3' },
    ]);
  });

  it('names the connection only while it is degraded, first, with what happens next', () => {
    const notice = connectionNotice('disconnected', 'test', 'short')!;
    expect(brandTooltipRows(1, [], notice)[0]).toEqual({ label: notice.title, value: notice.detail });
  });

  it('carries the version notices while no job runs', () => {
    updateAvailable.value = true;
    expect(brandTooltipRows(0, [], null)).toEqual([{ label: 'Client update', value: 'available' }]);
    expect(brandTooltipRows(0, [batch], null)).toEqual([{ label: 'Applying 14 changes', value: '1 done' }]);
  });
});

function findByClass(node: ComponentChildren, cls: string, out: VNode[] = []): VNode[] {
  if (node === null || node === undefined || typeof node === 'boolean') return out;
  if (typeof node === 'string' || typeof node === 'number') return out;
  if (Array.isArray(node)) {
    for (const child of node) findByClass(child, cls, out);
    return out;
  }
  const v = node as VNode<{ class?: string; className?: string; children?: ComponentChildren }>;
  const classAttr = v.props?.class ?? v.props?.className ?? '';
  if (typeof classAttr === 'string' && classAttr.split(/\s+/).includes(cls)) out.push(v);
  findByClass(v.props?.children, cls, out);
  return out;
}

describe('BrandBadge', () => {
  beforeEach(() => {
    engineRestarting.value = false;
    engineBuilding.value = false;
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    updateAvailable.value = false;
    enginePackaged.value = false;
    restartRequired.value = false;
    embeddingModelStatus.value = null;
    applyAllInProgress.value = false;
  });

  it('renders nothing when there is nothing to report', () => {
    expect(BrandBadge()).toBeNull();
  });

  it.each([
    ['busy', () => { engineBuilding.value = true; }],
    ['ready', () => { engineVersionReady.value = true; }],
    ['pending', () => { engineVersionPending.value = true; }],
  ])('is click-through with the correct layout in the %s state', (_state, arrange) => {
    arrange();
    const badge = findByClass(BrandBadge(), 'brand-badge')[0] as VNode<Record<string, unknown>> | undefined;
    expect(badge?.type).toBe(_state === 'ready' ? GlyphBadge : 'span');
    expect(String(badge?.props.class).includes('badge-icon')).toBe(_state === 'busy');
    expect(badge?.props.onClick).toBeUndefined();
  });

  it('spins while an Apply All runs', () => {
    applyAllInProgress.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-spinner')).toHaveLength(1);
  });

  it('draws a still queued glyph instead of the spinner while the build is queued', () => {
    engineBuilding.value = true;
    engineBuildDetail.value = {
      elapsedMs: 0,
      anchoredAt: 0,
      pendingCommits: null,
      queuedBehind: ['make lint'],
    };
    expect(findByClass(BrandBadge(), 'brand-badge-spinner')).toHaveLength(0);
    expect(findByClass(BrandBadge(), 'brand-badge-queued')).toHaveLength(1);
    expect(findByClass(BrandBadge(), 'badge-icon')).toHaveLength(1);
    engineBuildDetail.value = null;
  });

  it('draws no glyph in the pending state', () => {
    engineVersionPending.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-dot')).toHaveLength(1);
    expect(findByClass(BrandBadge(), 'brand-badge-spinner')).toHaveLength(0);
  });


  it('remounts the spinner whenever the visible mobile pane changes', () => {
    engineBuilding.value = true;
    const spinnerKey = () => findByClass(BrandBadge(), 'brand-badge-spinner')[0]?.key;
    mobileView.value = 'thread';
    const onThread = spinnerKey();
    mobileView.value = 'threads';
    const onThreads = spinnerKey();
    expect(onThread).toBeDefined();
    expect(onThreads).not.toBe(onThread);
  });
});

describe('UnreadBrandBadge', () => {
  beforeEach(() => {
    servedUnreadNotifications.value = { status: 'loaded', data: [] };
    peerWorkspaces.value = [];
    remotePeerWorkspaces.value = [];
  });

  function unread(): VNode<{ 'aria-label'?: string }> | undefined {
    return findByClass(UnreadBrandBadge(), 'brand-unread-badge')[0] as
      | VNode<{ 'aria-label'?: string }>
      | undefined;
  }

  function notes(n: number): Notification[] {
    return Array.from({ length: n }, (_, i) => ({
      id: `n${i}`,
      title: 't',
      message: 'm',
      read: false,
      created_at: '2026-01-01T00:00:00Z',
    })) as Notification[];
  }

  it('renders nothing when everything is read', () => {
    expect(UnreadBrandBadge()).toBeNull();
  });

  it('shows the notification menu total', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: notes(2) };
    expect(unread()).toBeDefined();
    expect(notificationsMenuUnreadTotal.value).toBe(2);
  });

  it('renders remote unreads when the current workspace has none', () => {
    remotePeerWorkspaces.value = [{
      gateway: 'lab', unreachable: null,
      workspaces: [{ id: 'notes', name: 'Notes', health: 'healthy', unread_count: 9 }],
    }];
    expect(vnodeToText(UnreadBrandBadge())).toContain('>9</span>');
  });

  it('coexists with the engine state badge rather than replacing it', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: notes(3) };
    engineBuilding.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-spinner'),
      'the spinner must survive an unread count').toHaveLength(1);
    expect(unread(), 'and the count must survive the spinner').toBeDefined();
  });

  it('is purely visual: no name of its own, and no tooltip', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: notes(3) };
    const el = unread() as VNode<Record<string, unknown>> | undefined;
    expect(el?.props['aria-hidden']).toBe('true');
    expect(el?.props['data-tooltip']).toBeUndefined();
    expect(el?.props['aria-label']).toBeUndefined();
  });

  it('phrases the count for the mark to speak, singular and plural', () => {
    expect(unreadBadgeLabel(0)).toBeNull();
    expect(unreadBadgeLabel(1)).toBe('1 unread notification');
    expect(unreadBadgeLabel(4)).toBe('4 unread notifications');
  });

  it('caps at the same number the menu rows do', () => {
    servedUnreadNotifications.value = { status: 'loaded', data: notes(100) };
    expect(vnodeToText(UnreadBrandBadge())).toContain('99+');
  });
});
