import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { brandBadgeState, brandBadgeTooltip, BrandBadge, UnreadBrandBadge, unreadBadgeLabel } from './BrandBadge';
import { vnodeToText } from '../chat/__tests__/vnodeToText';
import { crossWorkspaceUnreadTotal, peerWorkspaces } from '../../store/actions/app-badge';
import type { Notification } from '../../store/types';
import {
  unreadNotifications,
  restartRequired,
  engineVersionReady,
  engineVersionPending,
  engineRebuildWedged,
  engineBuilding,
  enginePackaged,
  updateAvailable,
  embeddingModelStatus,
  toasts,
  engineRestarting,
  focusedPane,
} from '../../store/store';
import type { BackgroundActivity, EngineBuildDetail } from '../../store/backgroundActivity';
import { backgroundActivities, activityToastContent } from '../../store/backgroundActivity';
import type { EmbeddingModelStatus } from '../../api/types';
import {
  BACKGROUND_ACTIVITY_TOAST_KEY,
  resetBackgroundActivityToastForTest,
} from '../../store/actions/backgroundActivity';

describe('brandBadgeState / brandBadgeTooltip', () => {
  /** The badge is driven by the count of in-flight background activities, which
   *  the component derives via `backgroundActivities`. Its own derivation is
   *  covered in `store/backgroundActivity.test.ts`; here the count is the input. */
  /** An engine build as it normally arrives: an elapsed counter, and the commits
   *  it will bring. Both live only in the toast, which is what earns the tap. */
  const build: BackgroundActivity = {
    kind: 'engine-build',
    label: 'Building new version',
    detail: '2m 14s',
    note: '2 commits since your running version\n• fix: one\n• docs: two',
    progress: null,
  };
  /** The degenerate case the conditional promise exists for: a build this client
   *  can say nothing more about (a co-located peer's, with git unable to answer).
   *  Tapping it would open a toast reading exactly this tooltip. */
  const bareBuild: BackgroundActivity = {
    kind: 'engine-build',
    label: 'Building new version',
    progress: null,
  };
  const download: BackgroundActivity = {
    kind: 'embedding-model',
    label: 'Downloading embedding model',
    detail: '212 MB of 465 MB',
    progress: 0.45,
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
    expect(brandBadgeTooltip([])).toBeUndefined();
  });

  it('dev at Apply time (restart pending, build not ready) shows no engine badge', () => {
    restartRequired.value = true;
    expect(brandBadgeState(0)).toBe('none');
    expect(brandBadgeTooltip([])).toBeUndefined();
  });

  it('a background rebuild in flight shows the busy badge', () => {
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeTooltip([build])).toBe('Building new version · tap for details');
  });

  it('an embedding-model download shows the same busy badge', () => {
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeTooltip([download])).toBe('Downloading embedding model · tap for details');
  });

  it('concurrent activities are named together in one tooltip', () => {
    expect(brandBadgeState(2)).toBe('busy');
    expect(brandBadgeTooltip([build, download])).toBe(
      'Building new version · Downloading embedding model · tap for details',
    );
  });

  /** The reported bug: the tooltip promised details and the toast said exactly
   *  the same thing back. The promise is derived from the content now, so an
   *  activity carrying nothing but its label doesn't make one. */
  it('promises no details when the toast would only repeat the tooltip', () => {
    expect(brandBadgeTooltip([bareBuild])).toBe('Building new version');
  });

  /** One activity with something to show is enough: the tap is worth taking. */
  it('promises details when any concurrent activity has some', () => {
    expect(brandBadgeTooltip([bareBuild, download])).toBe(
      'Building new version · Downloading embedding model · tap for details',
    );
  });

  /** An action counts as a detail: the toast offers something the tooltip
   *  cannot, even with no extra prose. */
  it('an action-only activity still earns the promise', () => {
    const awaitingApproval: BackgroundActivity = {
      kind: 'tailscale-serve',
      label: 'Waiting for you to enable Serve on your tailnet',
      progress: null,
      action: { kind: 'open-url', label: 'Enable in Tailscale', url: 'https://example.test/serve' },
    };
    expect(brandBadgeTooltip([awaitingApproval])).toBe(
      'Waiting for you to enable Serve on your tailnet · tap for details',
    );
  });

  it('busy wins over a concurrently-ready signal (switch not offered until the work lands)', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeTooltip([build])).toBe('Building new version · tap for details');
  });

  it('dev with the rebuild ready shows the attention (!) badge', () => {
    engineVersionReady.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeTooltip([])).toBe('New version available');
  });

  it('engine-ready + client update available is still one attention badge', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeTooltip([])).toBe('New version available · Client update available');
  });

  it('client update alone (engine idle) shows the attention badge with the client tooltip', () => {
    updateAvailable.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeTooltip([])).toBe('Client update available');
  });

  /** New code in source with nothing built behind it. A state of its own, and
   *  the quietest of the three, because it is the one the user can do least
   *  about. */
  it('source ahead with nothing built shows the pending badge', () => {
    engineVersionPending.value = true;
    expect(brandBadgeState(0)).toBe('pending');
    expect(brandBadgeTooltip([])).toBe('New code pending · tap to rebuild');
  });

  /** The tooltip must not promise a Rebuild the toast withholds, the same rule
   *  the activity tooltip follows about promising details. */
  it('a wedged rebuild says so instead of offering one', () => {
    engineVersionPending.value = true;
    engineRebuildWedged.value = true;
    expect(brandBadgeState(0)).toBe('pending');
    expect(brandBadgeTooltip([])).toBe('New code pending · no rebuild can deliver it');
  });

  it('ready wins over pending: something you can take now beats something unbuilt', () => {
    engineVersionReady.value = true;
    engineVersionPending.value = true;
    expect(brandBadgeState(0)).toBe('ready');
    expect(brandBadgeTooltip([])).toBe('New version available');
  });

  it('busy wins over pending: a build in flight may yet resolve it', () => {
    engineVersionPending.value = true;
    expect(brandBadgeState(1)).toBe('busy');
    expect(brandBadgeTooltip([build])).toBe('Building new version · tap for details');
  });
});

/** The reported bug, pinned across both halves that drifted apart: *"tap for
 *  details" but the details say exactly the same*.
 *
 *  The tooltip and the toast are separate pure functions over the same activity
 *  list, which is how one came to promise what the other could not deliver. This
 *  ties them: whenever the tooltip says "tap for details", the toast must say
 *  strictly more than the tooltip already did, and whenever it doesn't, tapping
 *  must be the user's own idea. Neither function can be changed alone without
 *  this failing. */
describe('the badge tooltip never promises more than the toast delivers', () => {
  const NOW = 1_700_000_000_000;
  const downloading: EmbeddingModelStatus = {
    model_id: 'multilingual-e5-small',
    load_state: { kind: 'downloading', downloaded_bytes: 244_000_000, total_bytes: 488_000_000 },
  };
  const richBuild: EngineBuildDetail = {
    elapsedMs: 134_000,
    anchoredAt: NOW,
    pendingCommits: {
      total: 2,
      groups: [
        { kind: 'fixed', total: 1, descriptions: ['one'] },
        { kind: 'housekeeping', total: 1, descriptions: [] },
      ],
    },
  };
  /** A co-located peer's build: the badge spins, but this client has neither a
   *  clock for it nor an answer from git, so there is nothing more to show. */
  const bareBuild: EngineBuildDetail = { elapsedMs: null, anchoredAt: NOW, pendingCommits: null };

  const scenarios: Array<[string, boolean, EmbeddingModelStatus | null, EngineBuildDetail | null]> = [
    ['a build with an elapsed time and commits', true, null, richBuild],
    ['a build this client knows nothing more about', true, null, bareBuild],
    ['an embedding-model download', false, downloading, null],
    ['both at once', true, downloading, richBuild],
  ];

  it.each(scenarios)('%s', (_case, building, model, detail) => {
    const activities = backgroundActivities(building, model, null, detail, NOW);
    const tooltip = brandBadgeTooltip(activities) ?? '';
    const message = activityToastContent(building, model, null, false, detail, NOW)?.message ?? '';
    const labels = activities.map((a) => a.label).join(' · ');

    if (tooltip.endsWith(' · tap for details')) {
      expect(message).not.toBe(labels);
      expect(message.length).toBeGreaterThan(labels.length);
    } else {
      // No promise was made, so the toast may legitimately repeat the labels.
      expect(tooltip).toBe(labels);
    }
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
    toasts.value = [];
    engineRestarting.value = false;
    engineBuilding.value = false;
    engineVersionReady.value = false;
    engineVersionPending.value = false;
    engineRebuildWedged.value = false;
    updateAvailable.value = false;
    enginePackaged.value = false;
    restartRequired.value = false;
    embeddingModelStatus.value = null;
    focusedPane.value = 'thread';
    resetBackgroundActivityToastForTest();
  });

  function badgeButton(): VNode<{ onClick?: (e: unknown) => void }> | undefined {
    return findByClass(BrandBadge(), 'brand-badge-action')[0] as
      | VNode<{ onClick?: (e: unknown) => void }>
      | undefined;
  }

  it('renders nothing when there is nothing to report', () => {
    expect(BrandBadge()).toBeNull();
  });

  /** The `!` badge is passive: its click must keep falling through to its host,
   *  which opens the Lucidos menu where Refresh and Restart live. */
  it('is not interactive in the ready state', () => {
    engineVersionReady.value = true;
    expect(badgeButton()).toBeUndefined();
    expect(findByClass(BrandBadge(), 'brand-badge')).toHaveLength(1);
  });

  it('is a button while background activity runs', () => {
    engineBuilding.value = true;
    expect(badgeButton()).toBeDefined();
  });

  /** The badge sits INSIDE its host, whose onClick opens the Lucidos menu for a
   *  click on any child. Without stopPropagation the tap would pop that menu
   *  over the toast it just opened. */
  it('opens the status toast and swallows the click', () => {
    engineBuilding.value = true;
    let stopped = false;
    badgeButton()?.props.onClick?.({ stopPropagation: () => { stopped = true; } });
    expect(stopped).toBe(true);
    expect(toasts.value.find((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY)?.message).toBe(
      'Building new version',
    );
  });

  /** The swallow above also eats the brand wrapper's own `focusPane('thread')`,
   *  so the badge claims the Threads pane group itself. `showToast` freezes a new
   *  toast over the pane focused at that moment, and the badge lives in the
   *  thread header: without this the toast pins over the content pane. */
  it('claims the thread pane so the toast lands there, not over the content pane', () => {
    focusedPane.value = 'content';
    engineBuilding.value = true;
    badgeButton()?.props.onClick?.({ stopPropagation: () => {} });
    expect(focusedPane.value).toBe('thread');
    expect(toasts.value.find((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY)?.pane).toBe('thread');
  });

  /** The pending badge IS clickable, unlike the ready `!`. It has no home in the
   *  Lucidos menu to fall through to (Restart would respawn the same engine),
   *  and its toast is dismissable, so without a way back a persistent dot would
   *  be something the user cannot resolve. */
  it('is a clickable dot in the pending state, and re-opens the pending toast', () => {
    engineVersionPending.value = true;
    const badge = badgeButton();
    expect(badge).toBeDefined();
    expect(findByClass(BrandBadge(), 'brand-badge-dot')).toHaveLength(1);
    let stopped = false;
    badge?.props.onClick?.({ stopPropagation: () => { stopped = true; } });
    expect(stopped).toBe(true);
    expect(toasts.value.find((t) => t.key === 'engine-new-version')?.action?.label).toBe('Rebuild');
  });

  /** The dot draws no glyph: the box is the mark. A `!` here would be a second
   *  attention mark for the one state with nothing to act on. */
  it('draws no glyph in the pending state', () => {
    engineVersionPending.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-spinner')).toHaveLength(0);
  });

  it('tints the dot when rebuilding is wedged, and re-opens the toast that says so', () => {
    engineVersionPending.value = true;
    engineRebuildWedged.value = true;
    expect(findByClass(BrandBadge(), 'brand-badge-wedged')).toHaveLength(1);
    badgeButton()?.props.onClick?.({ stopPropagation: () => {} });
    const toast = toasts.value.find((t) => t.key === 'engine-new-version');
    expect(toast?.type).toBe('warning');
    expect(toast?.action?.label).toBe('OK');
  });

  it('narrates a download with its byte progress', () => {
    embeddingModelStatus.value = {
      model_id: 'multilingual-e5-small',
      load_state: { kind: 'downloading', downloaded_bytes: 250, total_bytes: 1000 },
    };
    badgeButton()?.props.onClick?.({ stopPropagation: () => {} });
    const toast = toasts.value.find((t) => t.key === BACKGROUND_ACTIVITY_TOAST_KEY);
    expect(toast?.message).toContain('Downloading embedding model');
    expect(toast?.progress).toBeCloseTo(0.25);
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
