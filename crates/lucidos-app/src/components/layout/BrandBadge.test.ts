import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { brandBadgeState, brandBadgeLabel, BrandBadge, UnreadBrandBadge, unreadBadgeLabel } from './BrandBadge';
import { vnodeToText } from '../chat/__tests__/vnodeToText';
import { crossWorkspaceUnreadTotal, peerWorkspaces } from '../../store/actions/app-badge';
import type { ActivityRow } from '../../store/actions/activityRows';
import type { Notification } from '../../store/types';
import {
  unreadNotifications,
  restartRequired,
  engineVersionReady,
  engineVersionPending,
  engineRebuildWedged,
  engineBuilding,
  engineBuildDetail,
  enginePackaged,
  updateAvailable,
  embeddingModelStatus,
  applyAllInProgress,
  engineRestarting,
  mobileView,
} from '../../store/store';

describe('brandBadgeState / brandBadgeLabel', () => {
  /** The badge is driven by the activity group's rows, which the component
   *  derives via `liveActivityRows`. Their derivation is covered in
   *  `store/actions/activityRows.test.ts`; here the rows are the input. */
  const build: ActivityRow = {
    key: 'engine-build',
    label: 'Building new version',
    detail: '2m 14s',
    body: { kind: 'background', activity: { kind: 'engine-build', label: 'Building new version', progress: null } },
  };
  const apply: ActivityRow = {
    key: 'apply-all',
    label: 'Applying 4 changes',
    detail: '1 of 4',
    body: {
      kind: 'apply-all',
      thread: { threadId: 't-1', changeId: 'c-1', reading: null, title: 'Card gate fix', phase: 'Applying changes' },
      position: { index: 1, total: 4 },
      progress: { done: 0, working: 0.25 },
      timeLeft: null,
      canceling: false,
    },
  };

  beforeEach(() => {
    restartRequired.value = false;
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    engineRebuildWedged.value = false;
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

  /** New code in source with nothing built behind it. A state of its own, and
   *  the quietest of the three, because it is the one the user can do least
   *  about. */
  it('source ahead with nothing built shows the pending badge', () => {
    engineVersionPending.value = true;
    expect(brandBadgeState(0)).toBe('pending');
    expect(brandBadgeLabel([])).toBe('New code pending');
  });

  it('a wedged rebuild says so', () => {
    engineVersionPending.value = true;
    engineRebuildWedged.value = true;
    expect(brandBadgeState(0)).toBe('pending');
    expect(brandBadgeLabel([])).toBe('New code pending · no rebuild can deliver it');
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

/** Walk a vnode tree, returning every node whose className contains `cls`.
 *  Mirrors the helper in `shared/__tests__/toast-progress.test.tsx`: these
 *  components are called as plain functions, with no DOM render. */
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
    engineRebuildWedged.value = false;
    updateAvailable.value = false;
    enginePackaged.value = false;
    restartRequired.value = false;
    embeddingModelStatus.value = null;
    applyAllInProgress.value = false;
  });

  it('renders nothing when there is nothing to report', () => {
    expect(BrandBadge()).toBeNull();
  });

  /** A tap anywhere on the mark opens the Lucidos menu, so the badge must never
   *  take a tap of its own. Every state renders a plain span. */
  it.each([
    ['busy', () => { engineBuilding.value = true; }],
    ['ready', () => { engineVersionReady.value = true; }],
    ['pending', () => { engineVersionPending.value = true; }],
  ])('is a plain span with no handler in the %s state', (_state, arrange) => {
    arrange();
    const badge = findByClass(BrandBadge(), 'brand-badge')[0] as VNode<Record<string, unknown>> | undefined;
    expect(badge?.type).toBe('span');
    expect(badge?.props.onClick).toBeUndefined();
  });

  /** An Apply All is a job in flight like a build, so the badge spins for it. */
  it('spins while an Apply All runs', () => {
    applyAllInProgress.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-spinner')).toHaveLength(1);
  });

  /** A queued build is waiting, not working, so the badge must not spin. */
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
    engineBuildDetail.value = null;
  });

  /** The dot draws no glyph: the box is the mark. A `!` here would be a second
   *  attention mark for the one state with nothing to act on. */
  it('draws no glyph in the pending state', () => {
    engineVersionPending.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-dot')).toHaveLength(1);
    expect(findByClass(BrandBadge(), 'brand-badge-spinner')).toHaveLength(0);
  });

  it('tints the dot when rebuilding is wedged', () => {
    engineVersionPending.value = true;
    engineRebuildWedged.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-wedged')).toHaveLength(1);
  });

  /** The reported bug: on iOS the spinner froze after a swipe to the Threads
   *  pane. Each mobile header carries its own badge, and the hidden ones are
   *  `display: none`. WebKit can leave a spin frozen on an element that comes
   *  back from that, so every pane swap must hand the spinner a new element. */
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

/** The unread count, the mark's SECOND badge and the only in-app mirror of the
 *  app-icon badge. A separate component from `BrandBadge` on purpose. Folding a
 *  count into that ladder would hide the rebuild spinner while anything is
 *  unread, which on a dev workspace is most of the time. */
describe('UnreadBrandBadge', () => {
  beforeEach(() => {
    unreadNotifications.value = { status: 'loaded', data: [] };
    peerWorkspaces.value = [];
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

  it('shows the same total the app icon carries', () => {
    unreadNotifications.value = { status: 'loaded', data: notes(2) };
    expect(unread()).toBeDefined();
    // The computed is the single source both surfaces read, so asserting the
    // rendered number IS asserting the icon's.
    expect(crossWorkspaceUnreadTotal.value).toBe(2);
  });

  it('coexists with the engine state badge rather than replacing it', () => {
    // The reported failure this pins: a rebuild that shows no spinner because a
    // notification happens to be unread.
    unreadNotifications.value = { status: 'loaded', data: notes(3) };
    engineBuilding.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-spinner'),
      'the spinner must survive an unread count').toHaveLength(1);
    expect(unread(), 'and the count must survive the spinner').toBeDefined();
  });

  it('is purely visual: no name of its own, and no tooltip', () => {
    // Both would be dead. The badge is `pointer-events: none`, so `useTooltip`
    // (which walks UP from the hovered element) can never resolve it, and the
    // hover lands on the mark instead. The MARK speaks the count.
    unreadNotifications.value = { status: 'loaded', data: notes(3) };
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
    // One shared `countLabel`, so the mark and the rows cannot start eliding at
    // different counts.
    unreadNotifications.value = { status: 'loaded', data: notes(100) };
    expect(vnodeToText(UnreadBrandBadge())).toContain('99+');
  });
});
