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
import { useEffect } from 'preact/hooks';
import type { Loadable } from '../../store/types';
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
  type WorkspaceStatus,
  type GwRestoreStatus,
} from '../../api/client/control';

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

function PauseIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <rect x="6.5" y="5" width="4" height="14" rx="1.2" />
      <rect x="13.5" y="5" width="4" height="14" rx="1.2" />
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

function PencilIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
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

export function WorkspacePicker() {
  const workspaces = useSignal<Loadable<WorkspaceStatus[]>>({ status: 'not-loaded' });
  const busy = useSignal(false);
  const error = useSignal<string | null>(null);
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

  async function fetchWorkspaces(): Promise<void> {
    const list = await listWorkspaces();
    workspaces.value = { status: 'loaded', data: list };
    error.value = null;
  }

  async function fetchRestoreStatus(): Promise<void> {
    restoreStatus.value = await getRestoreStatus();
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
  }

  useEffect(() => {
    workspaces.value = { status: 'loading' };
    void refresh();
    // Seed the restore banner so a reload mid-restore re-attaches to the live
    // phase (the gateway holds the authoritative single-slot status).
    void fetchRestoreStatus().catch(() => { /* no banner until the poll lands */ });
    // Poll the full list so a workspace launched (or stopped) while the picker
    // is open appears on its own — not only while something is already
    // "Starting…" (that gate left a freshly-launched workspace invisible until a
    // manual reload). The same tick refreshes the restore status.
    const timer = setInterval(() => {
      void pollRefresh();
    }, 2000);
    return () => clearInterval(timer);
  }, []);

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

  const v = workspaces.value;
  // Predict a slug collision so the picker can warn before uploading (the
  // gateway re-checks authoritatively). Compares the typed name's slug against
  // the loaded workspace ids.
  const existingIds = v.status === 'loaded' ? v.data.map((w) => w.id) : [];
  const restoreSlug = restoreName.value.trim() ? slugifyWorkspaceName(restoreName.value.trim()) : '';
  const restoreCollision = restoreSlug !== '' && existingIds.includes(restoreSlug);
  const restoreCanSubmit =
    restoreFile.value != null && restoreKey.value.trim() !== '' && restoreName.value.trim() !== '' && !restoreCollision;
  const restore = restoreStatus.value;

  return (
    <div class="ws-picker">
      <div class="ws-picker-shell">
        <header class="ws-picker-header">
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
        {(v.status === 'loading' || v.status === 'not-loaded') && (
          <div class="ws-picker-empty">Loading…</div>
        )}

        {v.status === 'loaded' && v.data.length === 0 && (
          <div class="ws-picker-empty">No workspaces yet — create your first one.</div>
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
                      aria-label={`Open ${w.name}`}
                      onClick={() => openWorkspace(w.id)}
                      onKeyDown={(e) => {
                        // Only the row itself opens on Enter/Space — a keydown
                        // bubbling up from a focused action button (rename, play,
                        // …) must not also navigate to the workspace.
                        if (e.target !== e.currentTarget) return;
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          openWorkspace(w.id);
                        }
                      }}
                    >
                      <span class={`ws-picker-dot ws-picker-dot-${state}`} title={w.last_error || STATE_LABEL[state]} />
                      <span class="ws-picker-name">{w.name}</span>
                      <div class="ws-picker-actions" onClick={(e) => e.stopPropagation()}>
                        <button
                          class={`ws-picker-icon ws-picker-icon-toggle${w.autostart ? ' is-on' : ''}`}
                          disabled={busy.value}
                          title={w.autostart ? 'Starts when the gateway opens' : 'Start only when opened'}
                          aria-label={`${w.autostart ? 'Disable' : 'Enable'} start with gateway for ${w.name}`}
                          aria-pressed={w.autostart}
                          onClick={(e) => {
                            e.stopPropagation();
                            void withBusy(() => setAutostart(w.id, !w.autostart).then(refresh));
                          }}
                        ><ClockIcon /></button>
                        {running ? (
                          <button
                            class="ws-picker-icon ws-picker-icon-play"
                            disabled={busy.value}
                            title="Stop"
                            aria-label={`Stop ${w.name}`}
                            onClick={(e) => {
                              e.stopPropagation();
                              void withBusy(() => stopWorkspace(w.id).then(refresh));
                            }}
                          ><PauseIcon /></button>
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
                        <button
                          class="ws-picker-icon ws-picker-icon-secondary"
                          title="Rename"
                          aria-label={`Rename ${w.name}`}
                          onClick={(e) => { e.stopPropagation(); renamingId.value = w.id; renameValue.value = w.name; }}
                        ><PencilIcon /></button>
                        <button
                          class="ws-picker-icon ws-picker-icon-secondary ws-picker-icon-danger"
                          title="Delete"
                          aria-label={`Delete ${w.name}`}
                          onClick={(e) => { e.stopPropagation(); deletingId.value = w.id; deleteConfirm.value = ''; }}
                        ><TrashIcon /></button>
                      </div>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}

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
            <div class="ws-picker-inline">
              <input
                class="ws-picker-input"
                placeholder="Workspace name"
                value={newName.value}
                onInput={(e) => (newName.value = (e.target as HTMLInputElement).value)}
                onKeyDown={(e) => e.key === 'Enter' && onCreate()}
                autoFocus
              />
              <button class="ws-picker-btn ws-picker-btn-confirm" disabled={busy.value || !newName.value.trim()} onClick={onCreate}>
                {busy.value ? 'Creating…' : 'Create'}
              </button>
              <button class="ws-picker-btn" onClick={() => { creating.value = false; newName.value = ''; }}>Cancel</button>
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
