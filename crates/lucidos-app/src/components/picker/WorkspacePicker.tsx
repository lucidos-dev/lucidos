
import type { ComponentChildren } from 'preact';
import { useSignal } from '@preact/signals';
import { TrashSection, useTrashInventory } from './TrashSection';
import { deleteWorkspaces } from './workspaceDeletion';
import { ConfirmDialog } from '../shared/ConfirmDialog';
import { Disclosure } from '../shared/Disclosure';
import { Toast } from '../shared/Toast';
import { OverlayLayer } from '../layout/OverlayLayer';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { ChevronDownIcon, ChevronUpIcon, GripIcon, RetryIcon, SettingsIcon } from '../shared/icons';
import { LoadingFade } from '../shared/LoadingFade';
import { SkeletonProvider, SkText, SkBlock } from '../shared/Skeleton';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { useTooltip } from '../../hooks/useTooltip';
import { useWindowDragRegion } from '../../hooks/useWindowDragRegion';
import { useRowDrag } from '../../hooks/useRowDrag';
import { hasCoarsePointer } from '../../utils/viewport';
import { isInteractiveTarget } from '../../utils/dom';
import { errorDetail } from '../../utils/errorDetail';
import { dismissBootSplash } from '../../utils/bootSplash';
import { isTauri } from '../../utils/platform';
import { watchTitlebarBand } from '../../store/actions/trafficLights';
import { applyAppBadge } from '../../store/actions/app-badge';
import { WORKSPACE_ID, parseWorkspaceRoute } from '../../utils/basePath';
import {
  alternateOpenMode,
  defaultOpenMode,
  middleClickActivates,
  middleClickHandler,
  openModeForClick,
  openModeLabel,
  openWorkspaceIn,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import {
  recallLastWorkspace,
  forgetLastWorkspace,
  rememberLastWorkspaceCount,
  recallLastWorkspaceCount,
} from '../../utils/lastWorkspace';
import { isPairingShellDocument } from '../../utils/pairingShell';
import { hasUnspentPairingLaunch } from '../../utils/pairingCodeSeed';
import {
  listWorkspacesWithOrder,
  listRemoteWorkspaces,
  setWorkspaceOrder,
  createWorkspace,
  renameWorkspace,
  restartWorkspace,
  stopWorkspace,
  setAutostart,
  openWorkspace,
  restoreBackup,
  getRestoreStatus,
  clearRestoreStatus,
  getGatewayStatus,
  reloadGateway,
  getGatewayNetworkConfig,
  setGatewayNetworkConfig,
  type WorkspaceStatus,
  type GwRestoreStatus,
  type GatewayStatus,
} from '../../api/client/control';
import {
  toBindValue,
  isValidBindSelection,
  draftFromBind,
  type BindDraft,
} from '../../utils/bindMode';
import { workspaceFaultNote, workspaceState, workspaceStateLabel, type WorkspaceState } from '../../utils/workspaceState';
import { networkAccessBody, type NetworkEditor } from './NetworkAccessPopover';
import {
  applyRestoreFile,
  backupNote,
  createNote as buildCreateNote,
  isFirstRun,
  nameTakenBy,
  nameTakenMessage,
  restoreBlocker,
  restoreFileNote,
  showsAddress,
  workspaceAddress,
  EMPTY_RESTORE_DRAFT,
  type RestoreDraft,
} from './workspaceForms';
import { pickerFooter, type FooterMode } from './PickerFooter';
import { restoreBanner, restoreNeedsReading } from './RestoreBanner';
import {
  EMPTY_REMOTE_GATEWAY_DRAFT,
  remoteGatewayBlocker,
  submitRemoteGateway,
  type RemoteGatewayDraft,
} from '../settings/remoteGatewayForm';
import { GlyphBadge } from '../shared/GlyphBadge';
import { RemoteWorkspaceRowBody, UnreachableGatewayNotes } from './RemoteWorkspaceRow';
import {
  mergeRemoteListings,
  recallRemoteListings,
  rememberRemoteListings,
  remoteWorkspaceOpenable,
  type RemoteGatewayView,
} from './remoteWorkspaces';
import { keepUndrawnKeys, moveOrderKey, orderWorkspaces, stepOrderKey, type OrderedWorkspace } from '../../utils/workspaceOrder';
import { makeOrderSaver } from './orderSave';
import { singleFlight } from './singleFlight';

const REMOTE_POLL_EVERY_TICKS = 8;

const pickerCanDragStart = (target: HTMLElement): boolean =>
  !isInteractiveTarget(target) && !target.closest('[data-overlay-panel]');

const RESTORE_DONE_LINGER_MS = 6000;

const WORKSPACE_NAME_SUGGESTIONS = ['personal', 'work'] as const;

const MORE_BUTTON = '[data-role="ws-row-more"]';


const SPARK_D =
  'M0 -19 C2.5 -6 5.5 -2.5 18.5 0 C5.5 2.5 2.5 6 0 19 C-2.5 6 -5.5 2.5 -18.5 0 C-5.5 -2.5 -2.5 -6 0 -19 Z';

const revealDelay = (seconds: number) =>
  `animation-delay:calc(${seconds}s * var(--duration-scale))`;

function LucidosMark() {
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
          <rect class="ws-rv-sq" style={revealDelay(0.45)} x="-14.5" y="-14.5" width="29" height="29" rx="7" />
          <g transform="scale(0.6)"><path class="ws-rv-flash" style={revealDelay(0.15)} d={SPARK_D} /></g>
        </g>
        <g transform="translate(31.5 68.5)">
          <rect class="ws-rv-sq" style={revealDelay(0.85)} x="-14.5" y="-14.5" width="29" height="29" rx="7" />
          <g transform="scale(0.6)"><path class="ws-rv-flash" style={revealDelay(0.55)} d={SPARK_D} /></g>
        </g>
        <g transform="translate(68.5 68.5)">
          <rect class="ws-rv-sq" style={revealDelay(1.25)} x="-14.5" y="-14.5" width="29" height="29" rx="7" />
          <g transform="scale(0.6)"><path class="ws-rv-flash" style={revealDelay(0.95)} d={SPARK_D} /></g>
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

function PopOutIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <polyline points="15 3 21 3 21 9" />
      <line x1="10" y1="14" x2="21" y2="3" />
    </svg>
  );
}

function PopInIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <polyline points="12 6 12 12 18 12" />
      <line x1="21" y1="3" x2="12" y2="12" />
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

function PickerSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <SkeletonProvider>
      <ul class="ws-picker-list" aria-hidden="true">
        {Array.from({ length: rows }, (_, i) => (
          <li class="ws-picker-row" key={i}>
            <div class="ws-picker-open">
              <div class="ws-picker-line">
                <SkBlock w="0.625rem" h="0.625rem" circle />
                <div class="ws-picker-id">
                  <SkText class="ws-picker-name" w="9rem" />
                  <SkText class="ws-picker-backup" w="6rem" />
                </div>
                <div class="ws-picker-actions">
                  <SkBlock w="2rem" h="2rem" round />
                </div>
              </div>
            </div>
          </li>
        ))}
      </ul>
    </SkeletonProvider>
  );
}

type PickerEntry = OrderedWorkspace<WorkspaceStatus>;
type RemotePickerEntry = Extract<PickerEntry, { kind: 'remote' }>;

const NO_REMOTE_VIEWS: readonly RemoteGatewayView[] = [];

export function WorkspacePicker() {
  useTooltip();

  const workspaces = useSignal<Loadable<WorkspaceStatus[]>>({ status: 'not-loaded' });
  const remoteWorkspaces = useSignal<Loadable<RemoteGatewayView[]>>({ status: 'not-loaded' });
  const order = useSignal<readonly string[]>([]);
  const error = useSignal<string | null>(null);
  const [orderSaver] = useState(() => makeOrderSaver({
    draw: (drawn) => { order.value = drawn; },
    save: setWorkspaceOrder,
    report: (message) => { error.value = message; },
  }));
  const drawnEntries = useRef<PickerEntry[]>([]);
  const remoteAsk = useRef<((fresh?: boolean) => Promise<void>) | null>(null);
  const ticksSinceRemoteAsk = useRef(0);
  const busy = useSignal(false);
  const skeletonRowsRef = useRef<number | undefined>(undefined);
  const footerMode = useSignal<FooterMode>('none');
  const remoteDraft = useSignal<RemoteGatewayDraft>(EMPTY_REMOTE_GATEWAY_DRAFT);
  const pairedRemote = useSignal<string | null>(null);
  const remoteAwaitingOwner = useSignal(false);
  const newName = useSignal('');
  const renamingId = useSignal<string | null>(null);
  const renameValue = useSignal('');
  const deleting = useSignal(false);
  const trashRevision = useSignal(0);
  const restoreDraft = useSignal<RestoreDraft>(EMPTY_RESTORE_DRAFT);
  const restoreStatus = useSignal<GwRestoreStatus | null>(null);
  const gatewayStatus = useSignal<GatewayStatus | null>(null);
  const reloading = useSignal(false);
  const reloadConfirmOpen = useSignal(false);
  const reloadAnchor = useSignal<HTMLElement | null>(null);
  const networkOpen = useSignal(false);
  const networkAnchor = useSignal<HTMLElement | null>(null);
  const network = useSignal<Loadable<NetworkEditor>>({ status: 'not-loaded' });
  const networkSaving = useSignal(false);
  const networkLoadToken = useRef(0);
  const menuOpenId = useSignal<string | null>(null);
  const menuAnchor = useSignal<HTMLElement | null>(null);
  const selection = useSignal<ReadonlySet<string> | null>(null);
  const stopConfirmId = useSignal<string | null>(null);
  const stopAnchor = useSignal<HTMLElement | null>(null);
  const autoOpening = useSignal(
    typeof window !== 'undefined' &&
      !new URLSearchParams(window.location.search).has('pick') &&
      !isPairingShellDocument() &&
      !hasUnspentPairingLaunch() &&
      recallLastWorkspace() !== null,
  );
  const trashInventory = useTrashInventory(trashRevision.value, !autoOpening.value);

  async function fetchWorkspaces(): Promise<void> {
    const read = orderSaver.readStarted();
    const { workspaces: list, order: stored } = await listWorkspacesWithOrder();
    workspaces.value = { status: 'loaded', data: list };
    orderSaver.stored(stored, read);
    rememberLastWorkspaceCount(list.length);
    if (!isTauri()) {
      const total = list.reduce((sum, w) => sum + (w.unread_count ?? 0), 0);
      applyAppBadge(total);
    }
  }

  remoteAsk.current ??= singleFlight(async () => {
    ticksSinceRemoteAsk.current = 0;
    const listings = await listRemoteWorkspaces();
    const previous = remoteWorkspaces.value;
    remoteWorkspaces.value = {
      status: 'loaded',
      data: mergeRemoteListings(previous.status === 'loaded' ? previous.data : recallRemoteListings(), listings),
    };
    if (remoteWorkspaces.value.status === 'loaded') rememberRemoteListings(remoteWorkspaces.value.data);
  });
  const fetchRemoteWorkspaces = remoteAsk.current;

  async function refreshRemoteWorkspaces(): Promise<void> {
    try {
      await fetchRemoteWorkspaces(true);
    } catch (e) {
      remoteWorkspaces.value = toFailed(e);
    }
  }

  async function fetchRestoreStatus(): Promise<void> {
    restoreStatus.value = await getRestoreStatus();
  }

  async function fetchGatewayStatus(): Promise<void> {
    const status = await getGatewayStatus();
    gatewayStatus.value = status;
    if (reloading.value && !status.update_available) reloading.value = false;
  }

  async function refresh(): Promise<void> {
    try {
      await fetchWorkspaces();
    } catch (e) {
      workspaces.value = toFailed(e);
    }
  }

  async function pollRefresh(): Promise<void> {
    try {
      await fetchWorkspaces();
    } catch (e) {
      console.warn('[picker] background refresh failed; keeping last-good list', e);
    }
    if (++ticksSinceRemoteAsk.current >= REMOTE_POLL_EVERY_TICKS) {
      trashRevision.value++;
      try {
        await fetchRemoteWorkspaces();
      } catch (e) {
        console.warn('[picker] remote workspace poll failed; keeping last-good rows', e);
      }
    }
    try {
      await fetchRestoreStatus();
    } catch (e) {
      console.warn('[picker] restore-status poll failed; keeping last-good', e);
    }
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
    });
  }

  function loadNetwork() {
    const token = ++networkLoadToken.current;
    network.value = { status: 'loading' };
    void getGatewayNetworkConfig()
      .then((cfg) => {
        if (token !== networkLoadToken.current) return;
        network.value = {
          status: 'loaded',
          data: { config: cfg, draft: draftFromBind(cfg.gateway_bind, cfg.inherit) },
        };
      })
      .catch((e) => {
        if (token !== networkLoadToken.current) return;
        network.value = toFailed(e);
      });
  }

  function openNetwork(btn: HTMLElement) {
    networkAnchor.value = btn;
    networkOpen.value = true;
    loadNetwork();
  }

  function patchNetworkDraft(patch: Partial<BindDraft>) {
    const s = network.value;
    if (s.status !== 'loaded') return;
    network.value = {
      status: 'loaded',
      data: { ...s.data, draft: { ...s.data.draft, ...patch } },
    };
  }

  function fillDetectedTailscaleIp() {
    const s = network.value;
    if (s.status !== 'loaded') return;
    const ip = s.data.config.detected_tailscale_ip;
    if (!ip) return;
    patchNetworkDraft({ mode: 'address', address: ip });
  }

  function onSaveNetwork() {
    const s = network.value;
    if (s.status !== 'loaded') return;
    const { draft } = s.data;
    if (!isValidBindSelection(draft.mode, draft.address)) return;
    void withBusy(async () => {
      networkSaving.value = true;
      try {
        await setGatewayNetworkConfig({
          gateway_bind: toBindValue(draft.mode, draft.address),
          inherit: draft.inherit,
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
      if (remembered && parseWorkspaceRoute(remembered)?.gateway) {
        openWorkspace(remembered);
        return;
      }
      if (list.status === 'loaded' && remembered) {
        const w = list.data.find((x) => x.id === remembered);
        if (w && workspaceState(w) !== 'unhealthy') {
          openWorkspace(remembered);
          return;
        }
        if (!w) forgetLastWorkspace();
      }
      autoOpening.value = false;
    })();
    remoteWorkspaces.value = { status: 'loading' };
    void refreshRemoteWorkspaces();
    void fetchRestoreStatus().catch(() => {  });
    void fetchGatewayStatus().catch(() => {  });
    const timer = setInterval(() => {
      void pollRefresh();
    }, 2000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!autoOpening.value) dismissBootSplash();
  }, [autoOpening.value]);

  const firstRunPrompted = useSignal(false);
  useEffect(() => {
    if (firstRunPrompted.value) return;
    if (isFirstRun(workspaces.value) && footerMode.value === 'none') {
      firstRunPrompted.value = true;
      footerMode.value = 'create';
    }
  }, [workspaces.value]);

  const clearRetry = useSignal(0);
  const completedRestoreId =
    restoreStatus.value?.status === 'completed' && !restoreNeedsReading(restoreStatus.value)
      ? restoreStatus.value.id
      : null;
  useEffect(() => {
    if (!completedRestoreId) return;
    const timer = setTimeout(() => {
      void clearRestoreStatus()
        .then(() => {
          if (restoreStatus.value?.status === 'completed') restoreStatus.value = { status: 'idle' };
        })
        .catch((e) => {
          // BEST-EFFORT: self-recovering, per frontend.md's telemetry
          // carve-out. Nothing the user asked for failed (they pressed nothing),
          // and bumping the retry re-arms the timer for another attempt, so a
          // blip cannot strand a buttonless banner on screen.
          console.warn('[picker] clearing the finished restore failed; retrying', e);
          clearRetry.value++;
        });
    }, RESTORE_DONE_LINGER_MS);
    return () => clearTimeout(timer);
  }, [completedRestoreId, clearRetry.value]);

  const nameInputRef = useRef<HTMLInputElement>(null);
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
      error.value = errorDetail(e);
    } finally {
      busy.value = false;
    }
  }

  async function onDelete(ids: string[], bulk = false) {
    if (deleting.value) return;
    deleting.value = true;
    try {
      await deleteWorkspaces(ids, async () => {
        if (bulk) selection.value = null;
        trashRevision.value++;
        await refresh();
      });
    } finally {
      deleting.value = false;
    }
  }

  function onCreate() {
    const name = newName.value.trim();
    if (!name) return;
    void withBusy(async () => {
      const ws = await createWorkspace(name);
      footerMode.value = 'none';
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

  function pickRestoreFile(file: File | null | undefined) {
    restoreDraft.value = applyRestoreFile(restoreDraft.value, file);
  }

  function patchRestore(patch: Partial<RestoreDraft>) {
    restoreDraft.value = { ...restoreDraft.value, ...patch };
  }

  function closeRestore() {
    footerMode.value = 'none';
    restoreDraft.value = EMPTY_RESTORE_DRAFT;
  }

  function patchRemote(patch: Partial<RemoteGatewayDraft>) {
    remoteDraft.value = { ...remoteDraft.value, ...patch };
    pairedRemote.value = null;
  }

  function closeRemote() {
    footerMode.value = 'none';
    remoteDraft.value = EMPTY_REMOTE_GATEWAY_DRAFT;
    pairedRemote.value = null;
  }

  function onAddRemote() {
    if (remoteGatewayBlocker(remoteDraft.value) !== null) return;
    void withBusy(async () => {
      try {
        const added = await submitRemoteGateway(remoteDraft.value, () => {
          remoteAwaitingOwner.value = true;
        });
        remoteDraft.value = EMPTY_REMOTE_GATEWAY_DRAFT;
        pairedRemote.value = added.name;
        await refreshRemoteWorkspaces();
      } finally {
        remoteAwaitingOwner.value = false;
      }
    });
  }

  function onRestore() {
    const { file, key, name } = restoreDraft.value;
    if (!file || !key.trim() || !name.trim()) return;
    void withBusy(async () => {
      const started = await restoreBackup(file, key.trim(), name.trim());
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

  function openOrRetry(w: WorkspaceStatus, state: WorkspaceState, mode: WorkspaceOpenMode | null) {
    if (mode === null || w.health === 'restoring') return;
    if (state === 'unhealthy') {
      void withBusy(() => restartWorkspace(w.id).then(refresh));
      return;
    }
    void withBusy(async () => {
      await openWorkspaceIn(mode, w.id);
    });
  }

  function openRowMenu(e: MouseEvent, id: string) {
    if (hasCoarsePointer()) return;
    const trigger = (e.currentTarget as HTMLElement).querySelector<HTMLElement>(MORE_BUTTON);
    if (!trigger) return;
    e.preventDefault();
    menuAnchor.value = trigger;
    menuOpenId.value = id;
  }

  function reorder(reordered: readonly string[]) {
    void orderSaver.reorder(keepUndrawnKeys(order.value, reordered));
  }
  const drawnKeys = () => drawnEntries.current.map((entry) => entry.key);

  const { listRef, drag, rowProps, gripProps } = useRowDrag<HTMLUListElement>((from, to) => {
    const keys = drawnKeys();
    if (from !== to && keys[from] !== undefined) reorder(moveOrderKey(keys, keys[from], to));
  }, hasCoarsePointer);

  const v = workspaces.value;
  const remoteViews = remoteWorkspaces.value.status === 'loaded' ? remoteWorkspaces.value.data : NO_REMOTE_VIEWS;
  if (!drag) {
    drawnEntries.current = v.status === 'loaded' ? orderWorkspaces(v.data, remoteViews, order.value) : [];
  }
  const entries = drawnEntries.current;
  const selected = selection.value;
  const selectedIds = v.status === 'loaded' && selected
    ? v.data.filter((workspace) => selected.has(workspace.id)).map((workspace) => workspace.id)
    : [];

  function toggleSelected(id: string, checked: boolean) {
    const next = new Set(selection.value);
    if (checked) next.add(id);
    else next.delete(id);
    selection.value = next;
  }

  function rowMenu(id: string, label: string, disabled: boolean, items: ComponentChildren) {
    return (
      <div class="ws-picker-menu-wrap">
        <button
          class="ws-picker-icon ws-picker-icon-more"
          data-role="ws-row-more"
          disabled={disabled}
          data-tooltip="More"
          aria-label={label}
          aria-haspopup="menu"
          aria-expanded={menuOpenId.value === id}
          onClick={(e) => {
            e.stopPropagation();
            const btn = e.currentTarget as HTMLElement;
            if (menuOpenId.value === id) {
              menuOpenId.value = null;
            } else {
              menuAnchor.value = btn;
              menuOpenId.value = id;
            }
          }}
        ><MoreIcon /></button>
        <Overlay
          open={menuOpenId.value === id}
          onClose={() => {
            menuOpenId.value = null;
          }}
          anchor={menuAnchor.value}
          backdrop={false}
          panelClass="ws-picker-menu"
          panelRole="menu"
        >
          {items}
        </Overlay>
      </div>
    );
  }

  function moveItems(key: string) {
    const at = drawnKeys().indexOf(key);
    const step = (direction: 'up' | 'down') => (e: MouseEvent) => {
      e.stopPropagation();
      menuOpenId.value = null;
      reorder(stepOrderKey(drawnKeys(), key, direction));
    };
    return (
      <>
        <button class="ws-picker-menu-item" role="menuitem" disabled={at <= 0} onClick={step('up')}>
          <ChevronUpIcon size="1rem" />
          <span>Move up</span>
        </button>
        <button
          class="ws-picker-menu-item"
          role="menuitem"
          disabled={at < 0 || at === drawnKeys().length - 1}
          onClick={step('down')}
        >
          <ChevronDownIcon size="1rem" />
          <span>Move down</span>
        </button>
      </>
    );
  }

  function rowShell(entry: PickerEntry, index: number, draggable: boolean, body: ComponentChildren) {
    const workspace = selected && entry.kind === 'local' ? entry.workspace : null;
    draggable = draggable && !selected;
    const lift = drag?.from === index ? drag.dy : undefined;
    const dropAt = drag && drag.to === index && drag.to !== drag.from ? (drag.to > drag.from ? 'after' : 'before') : null;
    const unreachable = entry.kind === 'remote' && entry.view.unreachable !== null;
    const { onClick: dragClickGuard, ...holdProps } = rowProps;
    return (
      <li
        class={`ws-picker-row${unreachable ? ' ws-picker-row-unreachable' : ''}${lift !== undefined ? ' ws-picker-row-lifted' : ''}${dropAt ? ` ws-picker-row-drop-${dropAt}` : ''}`}
        key={entry.key}
        data-row-index={index}
        style={lift !== undefined ? { '--ws-picker-drag-dy': `${lift}px` } : undefined}
        {...(draggable ? { ...holdProps, onClickCapture: dragClickGuard } : {})}
      >
        {workspace && (
          <label class="ws-picker-select-slot">
            <input
              type="checkbox"
              aria-label={`Select ${workspace.name}`}
              checked={selected?.has(workspace.id)}
              disabled={deleting.value}
              onChange={(event) => toggleSelected(workspace.id, event.currentTarget.checked)}
            />
          </label>
        )}
        {draggable && (
          <span class="ws-picker-grip" aria-hidden="true" {...gripProps}>
            <GripIcon size="0.875rem" />
          </span>
        )}
        {body}
      </li>
    );
  }

  function remoteRow(entry: RemotePickerEntry, index: number) {
    const { view, workspace: w, key } = entry;
    const alternateMode = remoteWorkspaceOpenable(view, w) ? alternateOpenMode(workspaceState(w), WORKSPACE_ID, key) : null;
    const openRemote = (mode: WorkspaceOpenMode) => {
      void withBusy(async () => {
        await openWorkspaceIn(mode, key);
      });
    };
    return rowShell(entry, index, true, (
      <RemoteWorkspaceRowBody
        view={view}
        workspace={w}
        onOpen={(_route, mode) => openRemote(mode)}
        onContextMenu={(e) => openRowMenu(e, key)}
        actions={rowMenu(key, `More actions for ${w.name} on ${view.gateway}`, busy.value, (
          <>
            {alternateMode !== null && (
              <button
                class="ws-picker-menu-item"
                role="menuitem"
                disabled={busy.value}
                onClick={(e) => {
                  e.stopPropagation();
                  menuOpenId.value = null;
                  openRemote(alternateMode);
                }}
              >
                {alternateMode === 'separate' ? <PopOutIcon /> : <PopInIcon />}
                <span>{openModeLabel(alternateMode)}</span>
              </button>
            )}
            {moveItems(key)}
          </>
        ))}
      />
    ));
  }

  const listLoading = v.status === 'loading' || v.status === 'not-loaded';
  const trashStatus = trashInventory.state.status;
  const trashLoading = !autoOpening.value && (trashStatus === 'loading' || trashStatus === 'not-loaded');
  const showSkeletons = useDelayedFlag(listLoading || trashLoading);
  if (skeletonRowsRef.current === undefined) {
    skeletonRowsRef.current = recallLastWorkspaceCount() ?? 3;
  }
  const skeletonRows = skeletonRowsRef.current;
  const known = v.status === 'loaded' ? v.data : [];
  const blocker = restoreBlocker(restoreDraft.value, known);
  const fileNote = restoreFileNote(restoreDraft.value.file);
  const createNote = buildCreateNote(newName.value, known);
  const renameTaken = renamingId.value
    ? nameTakenBy(renameValue.value, known, renamingId.value)
    : null;
  const canRename = !busy.value && renameValue.value.trim() !== '' && renameTaken === null;
  const restore = restoreStatus.value;
  const restoreRunning = restore?.status === 'running';

  const [pickerEl, setPickerEl] = useState<HTMLElement | null>(null);
  const pickerRef = useMemo(() => ({ current: pickerEl }), [pickerEl]);
  useWindowDragRegion(pickerRef, { canStart: pickerCanDragStart });

  useEffect(() => (pickerEl ? watchTitlebarBand() : undefined), [pickerEl]);

  if (autoOpening.value) return null;

  return (
    <div class="ws-picker" ref={setPickerEl}>
      <div class="ws-picker-titlebar-band" data-titlebar-band aria-hidden="true" />
      <div class="ws-picker-shell">
        <header class="ws-picker-header">
          <button
            class="ws-picker-net-btn"
            disabled={busy.value}
            data-tooltip="Network access"
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
            <SettingsIcon />
          </button>
          <Overlay
            open={networkOpen.value}
            onClose={() => {
              networkOpen.value = false;
            }}
            anchor={networkAnchor.value}
            backdrop={false}
            panelClass="ws-picker-confirm ws-picker-net"
          >
            {networkAccessBody({
              state: network.value,
              saving: networkSaving.value,
              busy: busy.value,
              onMode: (mode) => patchNetworkDraft({ mode }),
              onAddress: (address) => patchNetworkDraft({ address }),
              onInherit: (inherit) => patchNetworkDraft({ inherit }),
              onFillDetected: fillDetectedTailscaleIp,
              onRetry: loadNetwork,
              onCancel: () => (networkOpen.value = false),
              onSave: onSaveNetwork,
            })}
          </Overlay>
          {gatewayStatus.value && !gatewayStatus.value.packaged && (
            <>
              <button
                class={`ws-picker-reload${gatewayStatus.value.update_available ? ' has-update' : ''}${reloading.value ? ' is-reloading' : ''}`}
                disabled={busy.value || reloading.value}
                data-tooltip={
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
                onClose={() => {
                  reloadConfirmOpen.value = false;
                }}
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
        </header>

        {error.value && <div class="ws-picker-error">{error.value}</div>}

        {v.status === 'failed' && (
          <div class="ws-picker-error">Failed to load workspaces: {v.error}</div>
        )}

        <LoadingFade showSkeleton={listLoading && showSkeletons} skeleton={<PickerSkeleton rows={skeletonRows} />}>
        {v.status === 'loaded' && (
          <>
          <Disclosure open={v.data.length > 0} bodyClass="ws-picker-select-bar">
            <div class={selected ? 'ws-picker-select-actions is-selecting' : 'ws-picker-select-actions'}>
              <button
                type="button"
                class="ws-picker-btn"
                aria-pressed={selected !== null}
                disabled={busy.value || deleting.value || Boolean(drag)}
                onClick={() => {
                  menuOpenId.value = null;
                  renamingId.value = null;
                  selection.value = selected ? null : new Set();
                }}
              >{selected ? 'Cancel selection' : 'Select'}</button>
              {selected && (
                <button
                  type="button"
                  class="ws-picker-btn ws-picker-btn-danger"
                  disabled={busy.value || deleting.value || selectedIds.length === 0}
                  onClick={() => void onDelete(selectedIds, true)}
                >Delete {selectedIds.length} {selectedIds.length === 1 ? 'workspace' : 'workspaces'}</button>
              )}
            </div>
          </Disclosure>
          <ul class="ws-picker-list" ref={listRef}>
            {entries.map((entry, index) => {
              if (entry.kind === 'remote') return remoteRow(entry, index);
              const w = entry.workspace;
              const state = workspaceState(w);
              const running = state === 'healthy' || state === 'booting';
              const fault = workspaceFaultNote(w);
              const backup = backupNote(w);
              const rowLabel = [
                w.health === 'restoring' ? `Restoring ${w.name}` : fault ? `Retry ${w.name} · ${fault}` : `Open ${w.name}`,
                w.health === 'restoring' ? w.last_error : null,
                backup?.text,
              ]
                .filter(Boolean)
                .join(' · ');
              const alternateMode = alternateOpenMode(state, WORKSPACE_ID, w.id);
              return rowShell(entry, index, renamingId.value !== w.id, renamingId.value === w.id ? (
                    <div class="ws-picker-rename">
                      <div class="ws-picker-inline">
                        <input
                          class="ws-picker-input"
                          value={renameValue.value}
                          onInput={(e) => (renameValue.value = (e.target as HTMLInputElement).value)}
                          onKeyDown={(e) => e.key === 'Enter' && canRename && onRename(w.id)}
                          autoFocus
                        />
                        <button
                          class="ws-picker-btn ws-picker-btn-confirm"
                          disabled={!canRename}
                          onClick={() => onRename(w.id)}
                        >Save</button>
                        <button class="ws-picker-btn" onClick={() => (renamingId.value = null)}>Cancel</button>
                      </div>
                      {renameTaken && (
                        <p class="ws-picker-note ws-picker-note-warn">{nameTakenMessage(renameTaken)}</p>
                      )}
                    </div>
                  ) : (
                    <div
                      class="ws-picker-open"
                      role="button"
                      tabIndex={0}
                      aria-label={rowLabel}
                      onClick={(e) => openOrRetry(w, state, openModeForClick(e))}
                      onAuxClick={middleClickActivates(state)
                        ? middleClickHandler((e) => openOrRetry(w, state, openModeForClick(e)))
                        : undefined}
                      onContextMenu={(e) => openRowMenu(e, w.id)}
                      onKeyDown={(e) => {
                        if (e.target !== e.currentTarget) return;
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          openOrRetry(w, state, defaultOpenMode());
                        }
                      }}
                    >
                      <div class="ws-picker-line">
                        <span
                          class={`ws-picker-dot ws-picker-dot-${state}`}
                          data-tooltip={workspaceStateLabel(w)}
                          aria-label={fault ? undefined : workspaceStateLabel(w)}
                          aria-hidden={fault ? 'true' : undefined}
                        />
                        <div class="ws-picker-id">
                          <span class="ws-picker-name">{w.name}</span>
                          {w.health === 'restoring' && <span class="ws-picker-address">{workspaceStateLabel(w)}</span>}
                          {showsAddress(w, v.data) && (
                            <span
                              class="ws-picker-address"
                              data-tooltip={`Served at ${workspaceAddress(w.id)}`}
                              aria-label={`Address ${workspaceAddress(w.id)}`}
                            >
                              {workspaceAddress(w.id)}
                            </span>
                          )}
                          <span
                            class={`ws-picker-backup${backup ? ` ws-picker-backup-${backup.level}` : ''}`}
                            aria-hidden="true"
                          >
                            {backup?.text ?? ''}
                          </span>
                        </div>
                        {typeof w.unread_count === 'number' && w.unread_count > 0 && (
                          <GlyphBadge
                            class="ws-picker-badge"
                            data-tooltip={`${w.unread_count} unread`}
                            aria-label={`${w.unread_count} unread notifications`}
                          >
                            {w.unread_count > 99 ? '99+' : w.unread_count}
                          </GlyphBadge>
                        )}
                        <div
                          class="ws-picker-actions"
                          onClick={(e) => e.stopPropagation()}
                          onAuxClick={(e) => e.stopPropagation()}
                        >
                          {running ? (
                            <div class="ws-picker-stop-wrap">
                              <button
                                class="ws-picker-icon ws-picker-icon-play ws-picker-icon-stop"
                                disabled={busy.value || w.health === 'restoring'}
                                data-tooltip="Stop"
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
                                onClose={() => {
                                  stopConfirmId.value = null;
                                }}
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
                                    disabled={busy.value || w.health === 'restoring'}
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
                              disabled={busy.value || w.health === 'restoring'}
                              data-tooltip={state === 'unhealthy' ? 'Retry' : 'Start'}
                              aria-label={`${state === 'unhealthy' ? 'Retry' : 'Start'} ${w.name}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                void withBusy(() => restartWorkspace(w.id).then(refresh));
                              }}
                            >{state === 'unhealthy' ? <RetryIcon /> : <PlayIcon />}</button>
                          )}
                          {rowMenu(w.id, `More actions for ${w.name}`, busy.value || w.health === 'restoring', (
                            <>
                              {alternateMode !== null && (
                                <button
                                  class="ws-picker-menu-item"
                                  role="menuitem"
                                  disabled={busy.value || w.health === 'restoring'}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    menuOpenId.value = null;
                                    void withBusy(async () => {
                                      await openWorkspaceIn(alternateMode, w.id);
                                    });
                                  }}
                                >
                                  {alternateMode === 'separate' ? <PopOutIcon /> : <PopInIcon />}
                                  <span>{openModeLabel(alternateMode)}</span>
                                </button>
                              )}
                              <button
                                class={`ws-picker-menu-item${w.autostart ? ' is-on' : ''}`}
                                role="menuitem"
                                disabled={busy.value || w.health === 'restoring'}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  menuOpenId.value = null;
                                  void withBusy(() => setAutostart(w.id, !w.autostart).then(refresh));
                                }}
                              >
                                <ClockIcon />
                                <span>{w.autostart ? 'Runs in the background' : 'Only runs while open'}</span>
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
                                disabled={deleting.value}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  menuOpenId.value = null;
                                  void onDelete([w.id]);
                                }}
                              >
                                <TrashIcon />
                                <span>Delete</span>
                              </button>
                              {moveItems(entry.key)}
                            </>
                          ))}
                        </div>
                      </div>
                      {fault && <p class="ws-picker-row-note">{fault}</p>}
                    </div>
                  ));
            })}
          </ul>
          </>
        )}
        </LoadingFade>

        {remoteWorkspaces.value.status === 'failed' && (
          <div class="ws-picker-error">
            Failed to load remote workspaces: {remoteWorkspaces.value.error}
          </div>
        )}
        <UnreachableGatewayNotes views={remoteViews} />

        {restoreBanner({ status: restore, busy: busy.value, onDismiss: onDismissRestore })}

        <TrashSection inventory={trashInventory} showSkeleton={trashLoading && showSkeletons} onChanged={refresh} />

        {pickerFooter({
          mode: footerMode.value,
          onMode: (m) => (footerMode.value = footerMode.value === m ? 'none' : m),
          busy: busy.value,
          restoreRunning,
          name: newName.value,
          onName: (n) => (newName.value = n),
          onCreate,
          onCancelCreate: () => { footerMode.value = 'none'; newName.value = ''; },
          suggestions: isFirstRun(v) ? WORKSPACE_NAME_SUGGESTIONS : [],
          onSuggestion: pickSuggestion,
          nameInputRef,
          createNote,
          draft: restoreDraft.value,
          onDraft: patchRestore,
          onPickFile: pickRestoreFile,
          onRestore,
          onCancelRestore: closeRestore,
          blocker,
          fileNote,
          onDeleteColliding: (id) => { void onDelete([id]); },
          remoteDraft: remoteDraft.value,
          onRemoteDraft: patchRemote,
          onAddRemote,
          onCancelRemote: closeRemote,
          pairedRemote: pairedRemote.value,
          remoteAwaitingOwner: remoteAwaitingOwner.value,
        })}
      </div>
      <OverlayLayer>
        <Toast />
        <ConfirmDialog />
      </OverlayLayer>
    </div>
  );
}
