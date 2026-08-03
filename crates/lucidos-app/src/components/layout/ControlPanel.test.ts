import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  currentWorkspaceRefreshState,
  controlPanelBadgeState,
  controlPanelBadgeTooltip,
  shouldActivateConfirm,
  BrandBadge,
} from './ControlPanel';
import {
  restartRequired,
  engineVersionReady,
  engineBuilding,
  enginePackaged,
  updateAvailable,
  engineNewVersionReady,
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

describe('currentWorkspaceRefreshState', () => {
  // Args: (ready, clientUpdateAvailable, enginePending)
  it('nothing pending — plain refresh tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, false, false);
    expect(tooltip).toBe('Refresh · hold to restart');
    expect(showUpdateBadge).toBe(false);
  });

  it('engine version ready, no client update — restart-and-apply tooltip, no dot', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(true, false, true);
    expect(tooltip).toBe('Refresh · hold to restart & apply changes');
    expect(showUpdateBadge).toBe(false);
  });

  it('client update available, no engine pending — prefixed tooltip, dot shown', () => {
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, true, false);
    expect(tooltip).toBe('Update available · Refresh · hold to restart');
    expect(showUpdateBadge).toBe(true);
  });

  it('client bundle rebuilt but engine still building — dot SUPPRESSED (refresh held until after switch)', () => {
    // The mixed-change build window: updateAvailable is true, but an engine
    // switch is pending, so the reload control must not advertise the refresh.
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(false, true, true);
    expect(showUpdateBadge).toBe(false);
    expect(tooltip).toBe('Refresh · hold to restart');
  });

  it('engine ready + client bundle newer — engine wins, client dot deferred', () => {
    // ready implies enginePending, so the client dot is held until after the switch.
    const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(true, true, true);
    expect(showUpdateBadge).toBe(false);
    expect(tooltip).toBe('Refresh · hold to restart & apply changes');
  });
});

describe('engineNewVersionReady — agrees with the background-build scheme', () => {
  beforeEach(() => {
    restartRequired.value = false;
    engineVersionReady.value = false;
    enginePackaged.value = false;
    updateAvailable.value = false;
  });

  it('dev: a freshly-applied restart-requiring change does NOT count until the build is ready', () => {
    // Apply time in dev: restartRequired flips true, but the background rebuild
    // hasn't finished — there is no new version to switch to yet.
    restartRequired.value = true;
    engineVersionReady.value = false;
    expect(engineNewVersionReady()).toBe(false);
  });

  it('dev: counts once the background rebuild flips engineVersionReady', () => {
    restartRequired.value = true;
    engineVersionReady.value = true;
    expect(engineNewVersionReady()).toBe(true);
  });

  it('packaged: a newer release (restartRequired from the outdated check) counts immediately', () => {
    // Packaged has no background build — engineVersionReady never fires there.
    enginePackaged.value = true;
    restartRequired.value = true;
    engineVersionReady.value = false;
    expect(engineNewVersionReady()).toBe(true);
  });

  it('packaged: up to date — no signal', () => {
    enginePackaged.value = true;
    restartRequired.value = false;
    expect(engineNewVersionReady()).toBe(false);
  });
});

describe('shouldActivateConfirm — the Restart-confirm double-click guard', () => {
  // A long-press reveals the Cancel/Restart confirm buttons UNDER the still-down
  // pointer. The browser then pairs a stray release `click` with the long-press;
  // on the mouse path that click lands on the freshly-mounted confirm button
  // (its pointerdown was on the now-unmounted refresh glyph, before the confirm
  // rendered). The old fix — a time-fused document-level `installPairedSwallow`
  // — ate the user's FIRST deliberate click instead, forcing a double-click.
  // The confirm now fires only when the click has its OWN preceding pointerdown
  // on the button (`freshPointerDown`) or is keyboard-synthesized.

  it('stray gesture-release click (no fresh pointerdown, not keyboard) is IGNORED', () => {
    expect(shouldActivateConfirm({ freshPointerDown: false, keyboard: false })).toBe(false);
  });

  it('a genuine tap fires on the FIRST click (its own pointerdown ran on the button)', () => {
    expect(shouldActivateConfirm({ freshPointerDown: true, keyboard: false })).toBe(true);
  });

  it('a keyboard activation (Enter/Space → click with no pointerdown) fires', () => {
    expect(shouldActivateConfirm({ freshPointerDown: false, keyboard: true })).toBe(true);
  });

  it('both signals present still fires (keyboard user who also moused down)', () => {
    expect(shouldActivateConfirm({ freshPointerDown: true, keyboard: true })).toBe(true);
  });
});

describe('controlPanelBadgeState / controlPanelBadgeTooltip', () => {
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
    engineBuilding.value = false;
    enginePackaged.value = false;
    updateAvailable.value = false;
  });

  it('nothing pending — no badge', () => {
    expect(controlPanelBadgeState(0)).toBe('none');
    expect(controlPanelBadgeTooltip([])).toBeUndefined();
  });

  it('dev at Apply time (restart pending, build not ready) shows no engine badge', () => {
    restartRequired.value = true;
    expect(controlPanelBadgeState(0)).toBe('none');
    expect(controlPanelBadgeTooltip([])).toBeUndefined();
  });

  it('a background rebuild in flight shows the busy badge', () => {
    expect(controlPanelBadgeState(1)).toBe('busy');
    expect(controlPanelBadgeTooltip([build])).toBe('Building new version · tap for details');
  });

  it('an embedding-model download shows the same busy badge', () => {
    expect(controlPanelBadgeState(1)).toBe('busy');
    expect(controlPanelBadgeTooltip([download])).toBe('Downloading embedding model · tap for details');
  });

  it('concurrent activities are named together in one tooltip', () => {
    expect(controlPanelBadgeState(2)).toBe('busy');
    expect(controlPanelBadgeTooltip([build, download])).toBe(
      'Building new version · Downloading embedding model · tap for details',
    );
  });

  /** The reported bug: the tooltip promised details and the toast said exactly
   *  the same thing back. The promise is derived from the content now, so an
   *  activity carrying nothing but its label doesn't make one. */
  it('promises no details when the toast would only repeat the tooltip', () => {
    expect(controlPanelBadgeTooltip([bareBuild])).toBe('Building new version');
  });

  /** One activity with something to show is enough: the tap is worth taking. */
  it('promises details when any concurrent activity has some', () => {
    expect(controlPanelBadgeTooltip([bareBuild, download])).toBe(
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
    expect(controlPanelBadgeTooltip([awaitingApproval])).toBe(
      'Waiting for you to enable Serve on your tailnet · tap for details',
    );
  });

  it('busy wins over a concurrently-ready signal (switch not offered until the work lands)', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(controlPanelBadgeState(1)).toBe('busy');
    expect(controlPanelBadgeTooltip([build])).toBe('Building new version · tap for details');
  });

  it('dev with the rebuild ready shows the attention (!) badge', () => {
    engineVersionReady.value = true;
    expect(controlPanelBadgeState(0)).toBe('ready');
    expect(controlPanelBadgeTooltip([])).toBe('New version available');
  });

  it('engine-ready + client update available is still one attention badge', () => {
    engineVersionReady.value = true;
    updateAvailable.value = true;
    expect(controlPanelBadgeState(0)).toBe('ready');
    expect(controlPanelBadgeTooltip([])).toBe('New version available · Client update available');
  });

  it('client update alone (engine idle) shows the attention badge with the client tooltip', () => {
    updateAvailable.value = true;
    expect(controlPanelBadgeState(0)).toBe('ready');
    expect(controlPanelBadgeTooltip([])).toBe('Client update available');
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
    pendingCommits: { total: 2, subjects: ['fix: one', 'docs: two'] },
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
    const tooltip = controlPanelBadgeTooltip(activities) ?? '';
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

  /** The `!` badge is passive: its click must keep falling through to the brand
   *  label, which opens the workspace switcher where Restart lives. */
  it('is not interactive in the ready state', () => {
    engineVersionReady.value = true;
    expect(badgeButton()).toBeUndefined();
    expect(findByClass(BrandBadge(), 'brand-badge')).toHaveLength(1);
  });

  it('is a button while background activity runs', () => {
    engineBuilding.value = true;
    expect(badgeButton()).toBeDefined();
  });

  /** The badge sits INSIDE `.pane-header-brand-label`, whose onClick opens the
   *  workspace switcher for a click on any child. Without stopPropagation the
   *  tap would pop that panel over the toast it just opened. */
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
