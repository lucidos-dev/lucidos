/**
 * Workspace picker (ADR 0014) — the gateway's picker surface, served at `/`
 * (smart root) and `/~/` with `<base href="/~/">`.
 *
 * Rendered standalone (see main.tsx — when `IS_PICKER`, i.e. the stamped base
 * href is `/~/`) instead of the full app, so it never boots the app's SSE /
 * thread machinery. It lists every registered workspace with health, and is the
 * always-reachable recovery surface: Open / Create / Rename / Delete / Retry.
 * A self-contained screen with inline forms — no global store, no native dialogs.
 *
 * Visual language: the Lucidos app mark is the hero (animated), painted on the
 * brand-blue gradient the mark itself uses. Rows are flat (no card chrome): the
 * whole row opens the workspace, start/stop is a single play/pause control, and
 * the secondary actions (auto-start / rename / delete) stay out of the way until
 * the row is hovered (always tappable on touch). The name lives in a flex-grow
 * cell with ellipsis so its length can never reshape the row.
 */

import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import type { Loadable } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { LoadingFade } from '../shared/LoadingFade';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { dismissBootSplash } from '../../utils/bootSplash';
import { isTauri } from '../../utils/platform';
import { applyAppBadge } from '../../store/actions/app-badge';
import {
  recallLastWorkspace,
  forgetLastWorkspace,
  rememberLastWorkspaceCount,
  recallLastWorkspaceCount,
} from '../../utils/lastWorkspace';
import {
  listWorkspaces,
  createWorkspace,
  renameWorkspace,
  deleteWorkspace,
  restartWorkspace,
  stopWorkspace,
  setAutostart,
  openWorkspace,
  restoreBackup,
  getRestoreStatus,
  clearRestoreStatus,
  slugifyWorkspaceName,
  parseWorkspaceNameFromArchive,
  getGatewayStatus,
  reloadGateway,
  getGatewayNetworkConfig,
  setGatewayNetworkConfig,
  type WorkspaceStatus,
  type GwRestoreStatus,
  type GatewayStatus,
  type GatewayNetworkConfig,
} from '../../api/client/control';
import {
  parseBindValue,
  toBindValue,
  isValidIp,
  isValidBindSelection,
  type BindMode,
} from '../../utils/bindMode';

/** Derived display state — collapses health + last_error into one status the row
 *  renders as a dot. A stopped workspace reports `unhealthy` + "not started"; we
 *  treat that as a calm "idle", distinct from a genuine failure. */
type PickerState = 'healthy' | 'booting' | 'stopped' | 'unhealthy';

function pickerState(w: WorkspaceStatus): PickerState {
  if (w.health === 'healthy') return 'healthy';
  if (w.health === 'booting') return 'booting';
  return w.last_error === 'not started' ? 'stopped' : 'unhealthy';
}

const STATE_LABEL: Record<PickerState, string> = {
  healthy: 'Ready',
  booting: 'Starting…',
  stopped: 'Stopped',
  unhealthy: 'Unhealthy',
};

/** Coarse phase labels the engine `restore-archive` CLI reports through the
 *  gateway's restore status. */
const RESTORE_PHASE_LABELS: Record<string, string> = {
  starting: 'Starting…',
  restoring: 'Restoring…',
  decrypting: 'Decrypting…',
  decompressing: 'Decompressing…',
  initializing: 'Unpacking files…',
  restoring_db: 'Restoring database…',
  done: 'Finishing…',
};

/** Quick-fill names offered in the create form while the name field is empty —
 *  the first-run "name your first workspace" nudge. Clicking one fills the
 *  (editable) field; the user still confirms with Create. */
export const WORKSPACE_NAME_SUGGESTIONS = ['personal', 'work'] as const;

/* ── Inline icons (kept local so the picker stays self-contained) ─────────── */

// The Lucidos sparkle, centered on the origin so it can be placed at any tile.
const SPARK_D =
  'M0 -19 C2.5 -6 5.5 -2.5 18.5 0 C5.5 2.5 2.5 6 0 19 C-2.5 6 -5.5 2.5 -18.5 0 C-5.5 -2.5 -2.5 -6 0 -19 Z';

function LucidosMark() {
  // The app icon (from public/favicon.svg) playing Logo Studio's "logo reveal":
  // a sparkle flashes at each tile in turn (TL → BL → BR → TR), leaving a square
  // behind it, and rests as the spark at the top-right — landing on the mark.
  return (
    <svg class="ws-picker-mark" viewBox="0 0 100 100" aria-hidden="true">
      <defs>
        <radialGradient id="wsBrand" gradientUnits="userSpaceOnUse" cx="30" cy="22" r="125">
          <stop offset="0" stop-color="#2d83e0" />
          <stop offset="1" stop-color="#0a4ea8" />
        </radialGradient>
      </defs>
      <rect x="0" y="0" width="100" height="100" rx="22" fill="url(#wsBrand)" />
      <g transform="translate(10 10) scale(0.8)" fill="#ffffff">
        <g transform="translate(31.5 31.5)">
          <rect class="ws-rv-sq" style="animation-delay:0.45s" x="-14.5" y="-14.5" width="29" height="29" rx="7" />
          <g transform="scale(0.6)"><path class="ws-rv-flash" style="animation-delay:0.15s" d={SPARK_D} /></g>
        </g>
        <g transform="translate(31.5 68.5)">
          <rect class="ws-rv-sq" style="animation-delay:0.85s" x="-14.5" y="-14.5" width="29" height="29" rx="7" />
          <g transform="scale(0.6)"><path class="ws-rv-flash" style="animation-delay:0.55s" d={SPARK_D} /></g>
        </g>
        <g transform="translate(68.5 68.5)">
          <rect class="ws-rv-sq" style="animation-delay:1.25s" x="-14.5" y="-14.5" width="29" height="29" rx="7" />
          <g transform="scale(0.6)"><path class="ws-rv-flash" style="animation-delay:0.95s" d={SPARK_D} /></g>
        </g>
        <g transform="translate(68.5 31)">
          <path class="ws-rv-final" d={SPARK_D} />
        </g>
      </g>
    </svg>
  );
}

function PlayIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M8 5.14v13.72a1 1 0 0 0 1.54.84l10.5-6.86a1 1 0 0 0 0-1.68L9.54 4.3A1 1 0 0 0 8 5.14Z" />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </svg>
  );
}

function ClockIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7.5V12l3 2" />
    </svg>
  );
}

function ReloadIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 12a9 9 0 0 1 15.5-6.2L21 8" />
      <path d="M21 3v5h-5" />
      <path d="M21 12a9 9 0 0 1-15.5 6.2L3 16" />
      <path d="M3 21v-5h5" />
    </svg>
  );
}

function GearIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z" />
    </svg>
  );
}

function PencilIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
    </svg>
  );
}

function MoreIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <circle cx="12" cy="5" r="1.8" />
      <circle cx="12" cy="12" r="1.8" />
      <circle cx="12" cy="19" r="1.8" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 6h18" />
      <path d="M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2" />
      <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
    </svg>
  );
}

/** Brand-matched loading placeholder for the workspace list — shimmer rows that
 *  mirror the real row's three-cell layout (dot · name · actions) so the
 *  skeleton→list handoff doesn't reflow. Kept local + self-contained like the
 *  rest of the picker; decorative → aria-hidden. Shown immediately while the
 *  list loads and crossfaded into it via `<LoadingFade>`. */
function PickerSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <ul class="ws-picker-list ws-picker-skel" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <li class="ws-picker-row" key={i}>
          <div class="ws-picker-open">
            <span class="ws-picker-skel-dot" />
            <span class="ws-picker-skel-name" />
            <span class="ws-picker-skel-action" />
          </div>
        </li>
      ))}
    </ul>
  );
}

export function WorkspacePicker() {
  const workspaces = useSignal<Loadable<WorkspaceStatus[]>>({ status: 'not-loaded' });
  const busy = useSignal(false);
  const error = useSignal<string | null>(null);
  // Skeleton row count, captured once at mount (see where it's read below).
  const skeletonRowsRef = useRef<number | undefined>(undefined);
  // Inline form state.
  const creating = useSignal(false);
  const newName = useSignal('');
  const renamingId = useSignal<string | null>(null);
  const renameValue = useSignal('');
  const deletingId = useSignal<string | null>(null);
  const deleteConfirm = useSignal('');
  // Restore-from-backup form + flow state.
  const restoreOpen = useSignal(false);
  const restoreFile = useSignal<File | null>(null);
  const restoreKey = useSignal('');
  const restoreName = useSignal('');
  const restoreStatus = useSignal<GwRestoreStatus | null>(null);
  // Gateway self-update: drives the header reload control + "new gateway
  // available" badge. Null until the first poll lands (legacy non-gateway mode
  // never resolves it, so the control stays hidden).
  const gatewayStatus = useSignal<GatewayStatus | null>(null);
  const reloading = useSignal(false);
  // Confirm step before re-execing the gateway (the reload re-execs the running
  // process). Anchored to the header reload icon.
  const reloadConfirmOpen = useSignal(false);
  const reloadAnchor = useSignal<HTMLElement | null>(null);
  // Machine-global Network access control (writes ~/.lucidos/network.toml): the
  // gateway bind + the "engines inherit gateway bind" toggle. Anchored to the
  // header gear. Lazy-loaded on open.
  const networkOpen = useSignal(false);
  const networkAnchor = useSignal<HTMLElement | null>(null);
  const networkConfig = useSignal<GatewayNetworkConfig | null>(null);
  const gwMode = useSignal<BindMode>('loopback');
  const gwAddress = useSignal('');
  const gwInherit = useSignal(true);
  const networkSaving = useSignal(false);
  // Per-row overflow menu (autostart / rename / delete). Only one open at a time;
  // anchor is the ⋯ button that opened it.
  const menuOpenId = useSignal<string | null>(null);
  const menuAnchor = useSignal<HTMLElement | null>(null);
  // Confirm step before stopping a running workspace (stopping it makes the
  // workspace unreachable until restarted). Anchored to the row's stop button.
  const stopConfirmId = useSignal<string | null>(null);
  const stopAnchor = useSignal<HTMLElement | null>(null);
  // Auto-open the last-active workspace whenever the picker loads — the smart
  // root (`/`) AND the sigil (`/~/`), since the installed PWA launches at the
  // picker manifest's `start_url` (`/~/`), not `/`. The ONLY escape is an
  // explicit `?pick`: the in-app "Manage workspaces" link carries it so the list
  // stays reachable for switching. Shows a brief "Opening…" splash while we
  // confirm the remembered workspace still exists; if it's gone we forget it and
  // fall through to the picker.
  const autoOpening = useSignal(
    typeof window !== 'undefined' &&
      !new URLSearchParams(window.location.search).has('pick') &&
      recallLastWorkspace() !== null,
  );

  async function fetchWorkspaces(): Promise<void> {
    const list = await listWorkspaces();
    workspaces.value = { status: 'loaded', data: list };
    error.value = null;
    // Remember the count so a future load can size its skeleton to this list
    // (no skeleton→list bounce on the next visit).
    rememberLastWorkspaceCount(list.length);
    // Gateway-root PWA app-icon badge: the AGGREGATE unread total across running
    // workspaces (a stopped one reports no count, so it contributes 0). Refreshed
    // every poll while the picker is open. Not under Tauri — the desktop process
    // drives a native dock badge from the same total (the WKWebView has no
    // installable app icon of its own).
    if (!isTauri()) {
      const total = list.reduce((sum, w) => sum + (w.unread_count ?? 0), 0);
      applyAppBadge(total);
    }
  }

  async function fetchRestoreStatus(): Promise<void> {
    restoreStatus.value = await getRestoreStatus();
  }

  async function fetchGatewayStatus(): Promise<void> {
    const status = await getGatewayStatus();
    gatewayStatus.value = status;
    // After a reload, the running gateway IS the on-disk binary, so the update
    // clears — that's our signal the new image is up; drop the "Reloading…" state.
    if (reloading.value && !status.update_available) reloading.value = false;
  }

  // Initial load + user-initiated actions: a failure surfaces the error screen.
  async function refresh(): Promise<void> {
    try {
      await fetchWorkspaces();
    } catch (e) {
      workspaces.value = { status: 'failed', error: String(e) };
    }
  }

  // Background poll (see useEffect). Best-effort, runs without user intent — a
  // transient blip keeps the last-good list rather than flashing the error
  // screen; the next tick recovers, and the initial load + user actions still
  // surface failures via refresh(). Per frontend.md's best-effort carve-out this
  // logs rather than toasts.
  async function pollRefresh(): Promise<void> {
    try {
      await fetchWorkspaces();
    } catch (e) {
      console.warn('[picker] background refresh failed; keeping last-good list', e);
    }
    // Best-effort restore-status poll (drives the restore banner / phase). A
    // blip just keeps the last-good status; the next tick recovers.
    try {
      await fetchRestoreStatus();
    } catch (e) {
      console.warn('[picker] restore-status poll failed; keeping last-good', e);
    }
    // Best-effort gateway self-update check (drives the reload control + badge).
    // Absent in legacy non-gateway mode and during a reload re-exec window; a
    // blip keeps the last-good status and the next tick recovers.
    try {
      await fetchGatewayStatus();
    } catch (e) {
      console.warn('[picker] gateway-status poll failed; keeping last-good', e);
    }
  }

  function onReloadGateway() {
    reloadConfirmOpen.value = false;
    void withBusy(async () => {
      reloading.value = true;
      await reloadGateway();
      // The gateway re-execs (~300ms delay server-side) and briefly drops; the
      // 2s poll reconnects and clears `reloading` once the new image answers.
    });
  }

  function openNetwork(btn: HTMLElement) {
    networkAnchor.value = btn;
    networkOpen.value = true;
    void getGatewayNetworkConfig()
      .then((cfg) => {
        networkConfig.value = cfg;
        const parsed = parseBindValue(cfg.gateway_bind);
        gwMode.value = parsed.mode;
        gwAddress.value = parsed.address;
        gwInherit.value = cfg.inherit;
      })
      .catch((e) => {
        error.value = String(e);
      });
  }

  // Click-to-fill the detected Tailscale address: drop it into the IP field and
  // switch to the Tailnet / IP mode (idempotent — already in that mode when the
  // detected line is visible, but set both so the affordance is self-contained).
  function fillDetectedTailscaleIp() {
    const ip = networkConfig.value?.detected_tailscale_ip;
    if (!ip) return;
    gwMode.value = 'address';
    gwAddress.value = ip;
  }

  function onSaveNetwork() {
    if (!isValidBindSelection(gwMode.value, gwAddress.value)) {
      error.value = 'Enter a valid IP address';
      return;
    }
    void withBusy(async () => {
      networkSaving.value = true;
      try {
        await setGatewayNetworkConfig({
          gateway_bind: toBindValue(gwMode.value, gwAddress.value),
          inherit: gwInherit.value,
        });
        networkOpen.value = false;
      } finally {
        networkSaving.value = false;
      }
    });
  }

  useEffect(() => {
    workspaces.value = { status: 'loading' };
    void (async () => {
      await refresh();
      if (!autoOpening.value) return;
      const list = workspaces.value;
      const remembered = recallLastWorkspace();
      if (list.status === 'loaded' && remembered) {
        const w = list.data.find((x) => x.id === remembered);
        // Only auto-open a workspace that isn't already known-unhealthy — opening
        // into an unhealthy engine is the dead-end we're fixing. 'stopped' /
        // 'booting' still auto-open (a stopped workspace lazy-starts, intended).
        if (w && pickerState(w) !== 'unhealthy') {
          openWorkspace(remembered); // navigates away; splash holds until unload
          return;
        }
        if (!w) forgetLastWorkspace(); // remembered workspace was deleted — stop retrying
        // else: it exists but is unhealthy — keep it remembered (not gone), but
        // fall through to the list so the user sees its state + Retry.
      }
      autoOpening.value = false; // nothing to open / unhealthy / load failed → show the picker
    })();
    // Seed the restore banner so a reload mid-restore re-attaches to the live
    // phase (the gateway holds the authoritative single-slot status).
    void fetchRestoreStatus().catch(() => { /* no banner until the poll lands */ });
    // Seed the gateway self-update status so the reload control appears without
    // waiting for the first 2s tick (absent in legacy non-gateway mode).
    void fetchGatewayStatus().catch(() => { /* no control until the poll lands */ });
    // Poll the full list so a workspace launched (or stopped) while the picker
    // is open appears on its own — not only while something is already
    // "Starting…" (that gate left a freshly-launched workspace invisible until a
    // manual reload). The same tick refreshes the restore status.
    const timer = setInterval(() => {
      void pollRefresh();
    }, 2000);
    return () => clearInterval(timer);
  }, []);

  // Drive the inline boot splash (index.html). While auto-opening, leave it up
  // untouched — it keeps its baked "Opening your workspace…" status (do NOT
  // re-set it here: the workspace document bakes the SAME text, so leaving it
  // alone keeps the status byte-identical across the cross-document hop, with no
  // wording swap at the seam). It stays on screen straight through the navigation
  // (the workspace's own inline splash continues it), so there is no picker-grid
  // "blink". When we instead show the list, fade it.
  useEffect(() => {
    if (!autoOpening.value) dismissBootSplash();
  }, [autoOpening.value]);

  // First run: when the picker loads with zero workspaces, open the create form
  // so the user names their first workspace right away — instead of the passive
  // "No workspaces yet" dead-end (and instead of a pre-made `default`). The field
  // stays empty and editable; the suggestion chips ("personal" / "work") offer a
  // one-click fill. Runs once; a manual Cancel then stays cancelled.
  const firstRunPrompted = useSignal(false);
  useEffect(() => {
    const list = workspaces.value;
    if (firstRunPrompted.value) return;
    if (list.status === 'loaded' && list.data.length === 0 && !creating.value && !restoreOpen.value) {
      firstRunPrompted.value = true;
      creating.value = true;
    }
  }, [workspaces.value]);

  // Ref to the create form's name input so a suggestion chip can fill it and
  // hand focus back (select the filled text so the user can type over it).
  const nameInputRef = useRef<HTMLInputElement | null>(null);
  function pickSuggestion(name: string) {
    newName.value = name;
    const el = nameInputRef.current;
    if (el) {
      el.focus();
      el.select();
    }
  }

  async function withBusy(fn: () => Promise<void>) {
    busy.value = true;
    error.value = null;
    try {
      await fn();
    } catch (e) {
      error.value = String(e);
    } finally {
      busy.value = false;
    }
  }

  function onCreate() {
    const name = newName.value.trim();
    if (!name) return;
    void withBusy(async () => {
      const ws = await createWorkspace(name);
      creating.value = false;
      newName.value = '';
      await refresh();
      openWorkspace(ws.id);
    });
  }

  function onRename(id: string) {
    const name = renameValue.value.trim();
    if (!name) return;
    void withBusy(async () => {
      await renameWorkspace(id, name);
      renamingId.value = null;
      await refresh();
    });
  }

  function onDelete(id: string) {
    void withBusy(async () => {
      await deleteWorkspace(id, deleteConfirm.value);
      deletingId.value = null;
      deleteConfirm.value = '';
      await refresh();
    });
  }

  function pickRestoreFile(file: File | null) {
    restoreFile.value = file;
    // Prefill the name from the archive filename (editable). Empty when the file
    // isn't a recognizable backup — the user then types one.
    restoreName.value = file ? (parseWorkspaceNameFromArchive(file.name) ?? '') : '';
  }

  function closeRestore() {
    restoreOpen.value = false;
    restoreFile.value = null;
    restoreKey.value = '';
    restoreName.value = '';
  }

  function onRestore() {
    const file = restoreFile.value;
    const key = restoreKey.value.trim();
    const name = restoreName.value.trim();
    if (!file || !key || !name) return;
    void withBusy(async () => {
      const started = await restoreBackup(file, key, name);
      // Optimistically show the running banner; the poll keeps it current.
      restoreStatus.value = { status: 'running', id: started.id, name: started.name, phase: 'starting' };
      closeRestore();
    });
  }

  function onDismissRestore() {
    void withBusy(async () => {
      await clearRestoreStatus();
      restoreStatus.value = { status: 'idle' };
    });
  }

  // Tapping a row opens the workspace — EXCEPT an `unhealthy` one, where opening
  // would drop the user into a dead app shell (the reported "navigated into a
  // workspace I couldn't connect to" bug). For unhealthy, Retry (restart +
  // refresh) instead — matching the row's play/Retry button — so the user opens
  // it once it goes healthy. `healthy`/`booting`/`stopped` open normally; a
  // stopped workspace lazy-starts behind the gateway's own auto-refreshing boot
  // splash, which is the good path, not the dead-skeleton case. This mirrors the
  // auto-open guard above, which already skips an unhealthy remembered workspace.
  function openOrRetry(w: WorkspaceStatus, state: PickerState) {
    if (state === 'unhealthy') {
      void withBusy(() => restartWorkspace(w.id).then(refresh));
      return;
    }
    openWorkspace(w.id);
  }

  const v = workspaces.value;
  // Gate the skeleton behind the standard SPINNER_DELAY_MS (300ms), like every
  // other view — a fast local gateway resolves well inside that window, so the
  // skeleton never appears and can't flash. There IS competing content behind it
  // (the brand header + footer render immediately) and the inline boot splash
  // fades over the picker for ~0.45s on every open, masking the brief empty→list
  // transition on a fast load — so the earlier "show it immediately" approach
  // (the picker as a no-competing-content exception) just produced a skeleton
  // blink under the clearing splash. Only a genuinely slow load (>300ms) now
  // shows the skeleton, which is exactly when it helps; <LoadingFade> still
  // crossfades it out so a shown skeleton doesn't hard-snap to the list.
  const listLoading = v.status === 'loading' || v.status === 'not-loaded';
  const showListSkeleton = useDelayedFlag(listLoading);
  // Size the skeleton to the last-known workspace count so the skeleton→list
  // handoff doesn't bounce (each skeleton row matches a real row's height).
  // Captured once at mount so it stays stable while the skeleton fades out
  // (LoadingFade) after the fresh count is recorded; first-ever visit → 3.
  if (skeletonRowsRef.current === undefined) {
    skeletonRowsRef.current = recallLastWorkspaceCount() ?? 3;
  }
  const skeletonRows = skeletonRowsRef.current;
  // Predict a slug collision so the picker can warn before uploading (the
  // gateway re-checks authoritatively). Compares the typed name's slug against
  // the loaded workspace ids.
  const existingIds = v.status === 'loaded' ? v.data.map((w) => w.id) : [];
  const restoreSlug = restoreName.value.trim() ? slugifyWorkspaceName(restoreName.value.trim()) : '';
  const restoreCollision = restoreSlug !== '' && existingIds.includes(restoreSlug);
  const restoreCanSubmit =
    restoreFile.value != null && restoreKey.value.trim() !== '' && restoreName.value.trim() !== '' && !restoreCollision;
  const restore = restoreStatus.value;

  // Smart-root auto-open in progress: render nothing so the inline boot splash
  // (index.html, kept up by the effect above) stays the only thing on screen —
  // the picker grid never flashes before the redirect to the last-active
  // workspace, and the splash carries straight through the navigation.
  if (autoOpening.value) return null;

  return (
    <div class="ws-picker">
      <div class="ws-picker-shell">
        <header class="ws-picker-header">
          <button
            class="ws-picker-net-btn"
            disabled={busy.value}
            title="Network access"
            aria-label="Network access"
            aria-haspopup="dialog"
            aria-expanded={networkOpen.value}
            onClick={(e) => {
              const btn = e.currentTarget as HTMLElement;
              if (networkOpen.value) {
                networkOpen.value = false;
              } else {
                openNetwork(btn);
              }
            }}
          >
            <GearIcon />
          </button>
          <Overlay
            open={networkOpen.value}
            onClose={() => (networkOpen.value = false)}
            anchor={networkAnchor.value}
            backdrop={false}
            panelClass="ws-picker-confirm ws-picker-net"
          >
            <h2 class="ws-picker-net-title">Network access</h2>
            <p class="ws-picker-net-desc">
              How this machine's Lucidos is reachable. The gateway fronts every
              workspace; engines can follow it or bind per-workspace.
            </p>
            <div class="ws-picker-net-modes" role="radiogroup" aria-label="Gateway bind">
              {(
                [
                  ['loopback', 'Loopback only'],
                  ['address', 'Tailnet / IP'],
                  ['all', 'All interfaces'],
                ] as [BindMode, string][]
              ).map(([m, label]) => (
                <button
                  key={m}
                  type="button"
                  role="radio"
                  aria-checked={gwMode.value === m}
                  class={`ws-picker-net-mode${gwMode.value === m ? ' active' : ''}`}
                  onClick={() => (gwMode.value = m)}
                >
                  {label}
                </button>
              ))}
            </div>
            {gwMode.value === 'address' && (
              <>
                <input
                  class="ws-picker-input"
                  type="text"
                  placeholder={networkConfig.value?.detected_tailscale_ip ?? '100.x.y.z'}
                  value={gwAddress.value}
                  aria-invalid={gwAddress.value.trim() !== '' && !isValidIp(gwAddress.value)}
                  aria-label="Gateway bind IP address"
                  onInput={(e) => (gwAddress.value = (e.target as HTMLInputElement).value)}
                />
                {gwAddress.value.trim() !== '' && !isValidIp(gwAddress.value) ? (
                  <span class="ws-picker-net-error">Not a valid IP address.</span>
                ) : networkConfig.value?.detected_tailscale_ip ? (
                  <span class="ws-picker-net-hint">
                    Detected Tailscale:{' '}
                    <button
                      type="button"
                      class="ws-picker-net-detected"
                      title="Use this address"
                      onClick={fillDetectedTailscaleIp}
                    >
                      {networkConfig.value.detected_tailscale_ip}
                    </button>
                  </span>
                ) : (
                  <span class="ws-picker-net-hint">Your Tailscale 100.x address, or a LAN IP.</span>
                )}
              </>
            )}
            <label class="ws-picker-net-toggle">
              <input
                type="checkbox"
                checked={gwInherit.value}
                onChange={(e) => (gwInherit.value = (e.target as HTMLInputElement).checked)}
              />
              <span>Engines inherit gateway bind</span>
            </label>
            <p class="ws-picker-net-hint">
              When off, each workspace sets its own engine bind in its Settings →
              Network access.
            </p>
            <p class="ws-picker-net-restart">
              Takes effect after the gateway / engine restarts.
            </p>
            <div class="ws-picker-confirm-actions">
              <button class="ws-picker-btn" onClick={() => (networkOpen.value = false)}>
                Cancel
              </button>
              <button
                class="ws-picker-btn ws-picker-btn-confirm"
                disabled={
                  busy.value ||
                  networkSaving.value ||
                  !networkConfig.value ||
                  (gwMode.value === 'address' &&
                    (gwAddress.value.trim() === '' || !isValidIp(gwAddress.value)))
                }
                onClick={onSaveNetwork}
              >
                {networkSaving.value ? 'Saving…' : 'Save'}
              </button>
            </div>
          </Overlay>
          {gatewayStatus.value && !gatewayStatus.value.packaged && (
            <>
              <button
                class={`ws-picker-reload${gatewayStatus.value.update_available ? ' has-update' : ''}${reloading.value ? ' is-reloading' : ''}`}
                disabled={busy.value || reloading.value}
                title={
                  reloading.value
                    ? 'Reloading gateway…'
                    : gatewayStatus.value.update_available
                      ? 'New gateway build available — reload to adopt it'
                      : 'Reload gateway'
                }
                aria-label="Reload gateway"
                onClick={(e) => {
                  const btn = e.currentTarget as HTMLElement;
                  if (reloadConfirmOpen.value) {
                    reloadConfirmOpen.value = false;
                  } else {
                    reloadAnchor.value = btn;
                    reloadConfirmOpen.value = true;
                  }
                }}
              >
                <ReloadIcon />
              </button>
              <Overlay
                open={reloadConfirmOpen.value}
                onClose={() => (reloadConfirmOpen.value = false)}
                anchor={reloadAnchor.value}
                backdrop={false}
                panelClass="ws-picker-confirm ws-picker-confirm-reload"
              >
                <p class="ws-picker-confirm-text">
                  Reload the gateway? Every running workspace stays up; the gateway
                  process re-execs and briefly drops.
                </p>
                <div class="ws-picker-confirm-actions">
                  <button class="ws-picker-btn" onClick={() => (reloadConfirmOpen.value = false)}>Cancel</button>
                  <button class="ws-picker-btn ws-picker-btn-confirm" disabled={busy.value} onClick={onReloadGateway}>Reload</button>
                </div>
              </Overlay>
            </>
          )}
          <div class="ws-picker-brand">
            <LucidosMark />
            <h1>Lucidos</h1>
          </div>
          <p>Choose a workspace</p>
        </header>

        {error.value && <div class="ws-picker-error">{error.value}</div>}

        {v.status === 'failed' && (
          <div class="ws-picker-error">Failed to load workspaces: {v.error}</div>
        )}

        <LoadingFade showSkeleton={showListSkeleton} skeleton={<PickerSkeleton rows={skeletonRows} />}>
        {v.status === 'loaded' && v.data.length === 0 && (
          <div class="ws-picker-empty">
            {creating.value
              ? 'Name your first workspace to get started.'
              : 'No workspaces yet — create your first one.'}
          </div>
        )}

        {v.status === 'loaded' && (
          <ul class="ws-picker-list">
            {v.data.map((w) => {
              const state = pickerState(w);
              const running = state === 'healthy' || state === 'booting';
              return (
                <li class="ws-picker-row" key={w.id}>
                  {renamingId.value === w.id ? (
                    <div class="ws-picker-inline">
                      <input
                        class="ws-picker-input"
                        value={renameValue.value}
                        onInput={(e) => (renameValue.value = (e.target as HTMLInputElement).value)}
                        onKeyDown={(e) => e.key === 'Enter' && onRename(w.id)}
                        autoFocus
                      />
                      <button class="ws-picker-btn ws-picker-btn-confirm" disabled={busy.value} onClick={() => onRename(w.id)}>Save</button>
                      <button class="ws-picker-btn" onClick={() => (renamingId.value = null)}>Cancel</button>
                    </div>
                  ) : deletingId.value === w.id ? (
                    <div class="ws-picker-inline ws-picker-delete">
                      <span>Type <strong>{w.name}</strong> to delete:</span>
                      <input
                        class="ws-picker-input"
                        value={deleteConfirm.value}
                        onInput={(e) => (deleteConfirm.value = (e.target as HTMLInputElement).value)}
                        onKeyDown={(e) => e.key === 'Enter' && deleteConfirm.value === w.name && onDelete(w.id)}
                        autoFocus
                      />
                      <button
                        class="ws-picker-btn ws-picker-btn-danger"
                        disabled={busy.value || deleteConfirm.value !== w.name}
                        onClick={() => onDelete(w.id)}
                      >Delete</button>
                      <button class="ws-picker-btn" onClick={() => { deletingId.value = null; deleteConfirm.value = ''; }}>Cancel</button>
                    </div>
                  ) : (
                    <div
                      class="ws-picker-open"
                      role="button"
                      tabIndex={0}
                      aria-label={state === 'unhealthy' ? `Retry ${w.name}` : `Open ${w.name}`}
                      onClick={() => openOrRetry(w, state)}
                      onKeyDown={(e) => {
                        // Only the row itself opens on Enter/Space — a keydown
                        // bubbling up from a focused action button (rename, play,
                        // …) must not also navigate to the workspace.
                        if (e.target !== e.currentTarget) return;
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          openOrRetry(w, state);
                        }
                      }}
                    >
                      <span class={`ws-picker-dot ws-picker-dot-${state}`} title={w.last_error || STATE_LABEL[state]} />
                      <span class="ws-picker-name">{w.name}</span>
                      {typeof w.unread_count === 'number' && w.unread_count > 0 && (
                        <span
                          class="ws-picker-badge"
                          title={`${w.unread_count} unread`}
                          aria-label={`${w.unread_count} unread notifications`}
                        >
                          {w.unread_count > 99 ? '99+' : w.unread_count}
                        </span>
                      )}
                      <div class="ws-picker-actions" onClick={(e) => e.stopPropagation()}>
                        {running ? (
                          <div class="ws-picker-stop-wrap">
                            <button
                              class="ws-picker-icon ws-picker-icon-play ws-picker-icon-stop"
                              disabled={busy.value}
                              title="Stop"
                              aria-label={`Stop ${w.name}`}
                              aria-haspopup="dialog"
                              aria-expanded={stopConfirmId.value === w.id}
                              onClick={(e) => {
                                e.stopPropagation();
                                const btn = e.currentTarget as HTMLElement;
                                if (stopConfirmId.value === w.id) {
                                  stopConfirmId.value = null;
                                } else {
                                  stopAnchor.value = btn;
                                  stopConfirmId.value = w.id;
                                }
                              }}
                            ><StopIcon /></button>
                            <Overlay
                              open={stopConfirmId.value === w.id}
                              onClose={() => (stopConfirmId.value = null)}
                              anchor={stopAnchor.value}
                              backdrop={false}
                              panelClass="ws-picker-confirm ws-picker-confirm-stop"
                            >
                              <p class="ws-picker-confirm-text">
                                Stop “{w.name}”? It shuts down and becomes unreachable
                                until you start it again.
                              </p>
                              <div class="ws-picker-confirm-actions">
                                <button
                                  class="ws-picker-btn"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    stopConfirmId.value = null;
                                  }}
                                >Cancel</button>
                                <button
                                  class="ws-picker-btn ws-picker-btn-danger"
                                  disabled={busy.value}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    stopConfirmId.value = null;
                                    void withBusy(() => stopWorkspace(w.id).then(refresh));
                                  }}
                                >Stop</button>
                              </div>
                            </Overlay>
                          </div>
                        ) : (
                          <button
                            class="ws-picker-icon ws-picker-icon-play"
                            disabled={busy.value}
                            title={state === 'unhealthy' ? 'Retry' : 'Start'}
                            aria-label={`${state === 'unhealthy' ? 'Retry' : 'Start'} ${w.name}`}
                            onClick={(e) => {
                              e.stopPropagation();
                              void withBusy(() => restartWorkspace(w.id).then(refresh));
                            }}
                          ><PlayIcon /></button>
                        )}
                        <div class="ws-picker-menu-wrap">
                          <button
                            class="ws-picker-icon ws-picker-icon-more"
                            disabled={busy.value}
                            title="More"
                            aria-label={`More actions for ${w.name}`}
                            aria-haspopup="menu"
                            aria-expanded={menuOpenId.value === w.id}
                            onClick={(e) => {
                              e.stopPropagation();
                              const btn = e.currentTarget as HTMLElement;
                              if (menuOpenId.value === w.id) {
                                menuOpenId.value = null;
                              } else {
                                menuAnchor.value = btn;
                                menuOpenId.value = w.id;
                              }
                            }}
                          ><MoreIcon /></button>
                          <Overlay
                            open={menuOpenId.value === w.id}
                            onClose={() => (menuOpenId.value = null)}
                            anchor={menuAnchor.value}
                            backdrop={false}
                            panelClass="ws-picker-menu"
                            panelRole="menu"
                          >
                            <button
                              class={`ws-picker-menu-item${w.autostart ? ' is-on' : ''}`}
                              role="menuitem"
                              disabled={busy.value}
                              onClick={(e) => {
                                e.stopPropagation();
                                menuOpenId.value = null;
                                void withBusy(() => setAutostart(w.id, !w.autostart).then(refresh));
                              }}
                            >
                              <ClockIcon />
                              <span>{w.autostart ? 'Starts with gateway' : 'Does not start with gateway'}</span>
                            </button>
                            <button
                              class="ws-picker-menu-item"
                              role="menuitem"
                              onClick={(e) => {
                                e.stopPropagation();
                                menuOpenId.value = null;
                                renamingId.value = w.id;
                                renameValue.value = w.name;
                              }}
                            >
                              <PencilIcon />
                              <span>Rename</span>
                            </button>
                            <button
                              class="ws-picker-menu-item ws-picker-menu-item-danger"
                              role="menuitem"
                              onClick={(e) => {
                                e.stopPropagation();
                                menuOpenId.value = null;
                                deletingId.value = w.id;
                                deleteConfirm.value = '';
                              }}
                            >
                              <TrashIcon />
                              <span>Delete</span>
                            </button>
                          </Overlay>
                        </div>
                      </div>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        </LoadingFade>

        {restore && restore.status === 'running' && (
          <div class="ws-picker-restore-banner" data-state="running">
            <span class="ws-picker-restore-spinner" />
            <span>Restoring “{restore.name}” — {RESTORE_PHASE_LABELS[restore.phase] || restore.phase}</span>
          </div>
        )}
        {restore && restore.status === 'completed' && (
          <div class="ws-picker-restore-banner" data-state="completed">
            <span>Restored “{restore.name}”</span>
            <button class="ws-picker-btn ws-picker-btn-confirm" onClick={() => openWorkspace(restore.id)}>Open</button>
            <button class="ws-picker-btn" disabled={busy.value} onClick={onDismissRestore}>Dismiss</button>
          </div>
        )}
        {restore && restore.status === 'failed' && (
          <div class="ws-picker-restore-banner" data-state="failed">
            <span>Restore failed: {restore.error}</span>
            <button class="ws-picker-btn" disabled={busy.value} onClick={onDismissRestore}>Dismiss</button>
          </div>
        )}

        <footer class="ws-picker-footer">
          {creating.value ? (
            <div class="ws-picker-create">
              {!newName.value.trim() && (
                <div class="ws-picker-suggestions">
                  <span class="ws-picker-suggestions-label">Try</span>
                  {WORKSPACE_NAME_SUGGESTIONS.map((s) => (
                    <button
                      key={s}
                      type="button"
                      class="ws-picker-suggestion"
                      disabled={busy.value}
                      onClick={() => pickSuggestion(s)}
                    >
                      {s}
                    </button>
                  ))}
                </div>
              )}
              <div class="ws-picker-inline">
                <input
                  ref={nameInputRef}
                  class="ws-picker-input"
                  placeholder="Workspace name"
                  value={newName.value}
                  onInput={(e) => (newName.value = (e.target as HTMLInputElement).value)}
                  onKeyDown={(e) => e.key === 'Enter' && onCreate()}
                  onFocus={(e) => (e.target as HTMLInputElement).select()}
                  autoFocus
                />
                <button class="ws-picker-btn ws-picker-btn-confirm" disabled={busy.value || !newName.value.trim()} onClick={onCreate}>
                  {busy.value ? 'Creating…' : 'Create'}
                </button>
                <button class="ws-picker-btn" onClick={() => { creating.value = false; newName.value = ''; }}>Cancel</button>
              </div>
            </div>
          ) : restoreOpen.value ? (
            <div class="ws-picker-restore-form">
              <label
                class="ws-picker-restore-drop"
                onDragOver={(e) => e.preventDefault()}
                onDrop={(e) => {
                  e.preventDefault();
                  pickRestoreFile(e.dataTransfer?.files?.[0] ?? null);
                }}
              >
                <input
                  type="file"
                  accept=".enc"
                  hidden
                  onChange={(e) => pickRestoreFile((e.target as HTMLInputElement).files?.[0] ?? null)}
                />
                <span>{restoreFile.value ? restoreFile.value.name : 'Drop a .enc backup here, or click to choose'}</span>
              </label>
              <input
                class="ws-picker-input"
                type="password"
                placeholder="Backup key"
                value={restoreKey.value}
                onInput={(e) => (restoreKey.value = (e.target as HTMLInputElement).value)}
              />
              <input
                class="ws-picker-input"
                placeholder="Workspace name"
                value={restoreName.value}
                onInput={(e) => (restoreName.value = (e.target as HTMLInputElement).value)}
                onKeyDown={(e) => e.key === 'Enter' && restoreCanSubmit && !busy.value && onRestore()}
              />
              {restoreCollision && (
                <span class="ws-picker-restore-warn">“{restoreName.value.trim()}” already exists — choose another name</span>
              )}
              <div class="ws-picker-inline">
                <button
                  class="ws-picker-btn ws-picker-btn-confirm"
                  disabled={!restoreCanSubmit || busy.value}
                  onClick={onRestore}
                >
                  {busy.value ? 'Starting…' : 'Restore'}
                </button>
                <button class="ws-picker-btn" onClick={closeRestore}>Cancel</button>
              </div>
            </div>
          ) : (
            <div class="ws-picker-footer-actions">
              <button class="ws-picker-new" onClick={() => (creating.value = true)}>+ New workspace</button>
              <button
                class="ws-picker-new ws-picker-restore-open"
                disabled={restore?.status === 'running'}
                onClick={() => (restoreOpen.value = true)}
              >
                Restore from backup
              </button>
            </div>
          )}
        </footer>
      </div>
    </div>
  );
}
