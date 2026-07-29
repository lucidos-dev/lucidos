/**
 * EntityReferenceManager — independent SSE consumer that keeps entity references
 * (recents, nav stack, pinned apps, active overlay) in sync when entities change.
 *
 * Wired at the SSE dispatch level in thread-sync.ts, NOT as a side-effect of
 * handleThreadEvent or handleGlobalEvent.
 */
import { panelOverlay, appsList, installedPlugins, triggers, credentials, environmentVariables, chatModels, oauthAccounts, repositories, artifacts, llmConfigured, configuredProviders } from '../store';
import { checkHealth } from '../../api/client';
import { loadApps } from './apps';
import { loadInstalledPlugins } from './plugins';
import { loadChatModels } from './models';
import { loadTriggers } from './triggers';
import { loadThreadQueue } from './threadQueue';
import { loadTriggerGroups } from './triggerGroups';
import { loadArtifacts } from './artifacts';
import { removePinnedAppLocal, loadPinnedApps } from './pinnedApps';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadOAuthAccounts } from './oauth';
import { loadRepositories } from './repositoriesLoader';
import { loadDevices, devices, getDeviceId } from './devices';

export const RECENTS_KEY = 'lucidos-search-recents';
export const NAV_KEY = 'lucidos-nav-history';

/** Re-probe `/health` and update `llmConfigured` after a provider credential
 *  change. The backend hot-swaps the active LLM provider in an in-process
 *  broadcast subscriber (`spawn_provider_credential_subscriber`), so there is a
 *  brief race between that swap and this probe: fire one immediate probe (it
 *  usually wins — the in-process rebuild beats the SSE→browser→`/health`
 *  round-trip) plus one short delayed re-check for the loser case. The 5s
 *  connection poll (`useStartup.ts` → `checkConnection`) is the guaranteed
 *  backstop, so first-run provider onboarding clears (or reappears on the last
 *  credential's removal) WITHOUT a manual page refresh.
 *
 *  Deliberately a minimal local probe rather than calling `connection.ts`'s
 *  `checkConnection`: importing connection here would close a circular import
 *  (`connection → thread-sync → entityReferences`). `checkHealth` never throws
 *  (returns a `Loadable`); a failed probe is left to the poll backstop — no
 *  toast, the credential save already reported its own result. */
async function probeLlmConfigured(): Promise<void> {
  const result = await checkHealth();
  if (result.status === 'loaded') {
    // Mirror connection.ts: only an explicit `false` flips to unconfigured.
    llmConfigured.value = result.data.llm_configured !== false;
    // Refresh the configured-provider set too, so the model picker filter
    // tracks the credential change that triggered this probe.
    if (result.data.configured_providers !== undefined) {
      configuredProviders.value = result.data.configured_providers;
    }
  }
}

function refreshLlmConfigured(): void {
  void probeLlmConfigured();
  setTimeout(() => {
    void probeLlmConfigured();
  }, 600);
}

/** Process a raw SSE message for entity reference updates.
 *
 *  Loaders below are fire-and-forget — each owns its own failure surface
 *  (Loadable failed state via toFailed, plus showToast where applicable in
 *  the loader). `void` is the explicit fire-and-forget marker so the loader's
 *  failure path is the single source of truth and we don't double-toast from
 *  here. */
export function processSSEForReferences(type: string, data: Record<string, unknown>): void {
  switch (type) {
    // Trigger events — only Created / Updated / Deleted can change a trigger's
    // group_id and thus the member_count badge on the section header. Enabled /
    // Disabled / Executed touch unrelated fields, so skip the group refetch on
    // those to keep the SSE pipeline cheap.
    case 'TriggerCreated':
    case 'TriggerUpdated':
      void loadTriggers();
      void loadTriggerGroups();
      break;
    case 'TriggerEnabled':
    case 'TriggerDisabled':
    case 'TriggerExecuted':
      void loadTriggers();
      break;
    // Thread Queue events — every transition (enqueue, admit, drop, complete)
    // and policy change re-fetches the panel state. The list is small (live
    // queue + active set only) so a full refetch per event stays cheap.
    // ThreadQueueChanged is the transient signal for in-memory-only
    // user-initiated slot moves (which carry no persisted ThreadQueue* event).
    case 'ThreadQueued':
    case 'ThreadQueueAdmitted':
    case 'ThreadQueueDropped':
    case 'ThreadQueueCompleted':
    case 'ThreadQueueChanged':
    case 'CapacityPolicyChanged':
      void loadThreadQueue();
      break;
    case 'TriggerDeleted': {
      const triggerId = data.trigger_id as string;
      if (triggerId) {
        pruneRecents(triggerId, 'triggers');
        pruneNavStack('trigger', triggerId);
        closeOverlayIfStale('trigger', triggerId);
      }
      void loadTriggers();
      void loadTriggerGroups();
      break;
    }
    // Trigger group lifecycle — pure organizational events; refresh the
    // group registry so the panel re-renders section headers.
    case 'TriggerGroupCreated':
    case 'TriggerGroupRenamed':
    case 'TriggerGroupReordered':
    case 'TriggerGroupDeleted':
      void loadTriggerGroups();
      break;
    // App events
    case 'AppCreated':
      void loadApps();
      break;
    case 'AppUpdated': {
      const appId = data.app_id as string;
      const name = data.name as string | undefined;
      if (appId && name) patchRecentsMetadata(appId, 'apps', name);
      void loadApps();
      break;
    }
    case 'AppDeleted': {
      const appId = data.app_id as string;
      if (appId) {
        pruneRecents(appId, 'apps');
        pruneNavStack('app', appId);
        removePinnedAppLocal(appId);
        closeOverlayIfStale('app', appId);
      }
      void loadApps();
      break;
    }
    // File events. `ArtifactImported` is the user-driven import flow;
    // `ArtifactCreated`/`Updated`/`Deleted` fire from `emit_entity_events_for_change_apply`
    // when a coding-agent change lands files in `data/artifacts/`. All four
    // refresh the same `artifacts` list — gated on `loaded` for the latter
    // three to match the PluginInstalled pattern (don't warm caches the user
    // hasn't opened), unconditional for ArtifactImported because the import
    // flow always has a settings panel open.
    case 'ArtifactImported':
      void loadArtifacts();
      break;
    case 'ArtifactCreated':
    case 'ArtifactUpdated':
    case 'ArtifactDeleted':
      if (artifacts.value.status === 'loaded') void loadArtifacts();
      break;
    // `RepositoryImported` is the `git_clone` tool landing a repo's files under
    // `data/artifacts/imported/<name>/` — a BULK ARTIFACT import, not a change
    // to the registered-repositories list (it carries url/destination/files,
    // no repo_id). So it refreshes the `artifacts` cache, same gating as the
    // Artifact* trio above. (Previously it wrongly reloaded the registered-repo
    // list, which never contains the clone — so an agent-imported repo never
    // appeared anywhere live.)
    case 'RepositoryImported':
      if (artifacts.value.status === 'loaded') void loadArtifacts();
      break;
    // Data-file mutations via the HTTP `/data/*` API (SDK `lucidos.data.*`,
    // `lucidos` CLI). The engine emits these but NOT a paired Artifact* event,
    // so a write/edit/delete under `data/artifacts/` would otherwise leave the
    // artifacts list stale until reload. `path` is relative to `data/`
    // (e.g. `artifacts/notes.md`); refresh the artifacts cache when it targets
    // the artifacts subtree. Gated on `loaded` like the Artifact* trio.
    case 'DataFileWritten':
    case 'DataFileEdited':
    case 'DataFileDeleted': {
      const dataPath = data.path as string | undefined;
      if (dataPath?.startsWith('artifacts/') && artifacts.value.status === 'loaded') {
        void loadArtifacts();
      }
      break;
    }
    // Plugin install/uninstall lands files under apps/, knowhow/, triggers/,
    // scripts/, auth-modules/. Only refresh lists the user has already
    // loaded — eagerly populating caches the user hasn't asked for is pure
    // network + render waste. Knowhow has no list view; scripts/auth-modules
    // don't surface.
    case 'PluginInstalled':
    case 'PluginUninstalled':
      if (appsList.value.status === 'loaded') void loadApps();
      if (triggers.value.status === 'loaded') void loadTriggers();
      if (installedPlugins.value.status === 'loaded') void loadInstalledPlugins();
      break;
    // Settings-page caches — gated on `status === 'loaded'` (same as Plugin*
    // above) so cross-device events don't warm caches the user hasn't asked
    // for. When the user opens the matching settings panel the loader runs
    // and fetches fresh data anyway.
    case 'CredentialCreated':
    case 'CredentialUpdated':
    case 'CredentialDeleted':
      if (credentials.value.status === 'loaded') void loadCredentials();
      // A provider credential change rebuilds + swaps the engine's active LLM
      // provider at runtime (no restart). Re-probe /health so `llmConfigured`
      // (and thus first-run provider onboarding) reflects the swap without a
      // manual page refresh.
      refreshLlmConfigured();
      break;
    case 'EnvironmentVariableSet':
    case 'EnvironmentVariableDeleted':
      if (environmentVariables.value.status === 'loaded') void loadEnvironmentVariables();
      break;
    case 'ModelCreated':
    case 'ModelUpdated':
    case 'ModelDeleted':
      if (chatModels.value.status === 'loaded') void loadChatModels();
      break;
    case 'OAuthAccountDeleted':
      if (oauthAccounts.value.status === 'loaded') void loadOAuthAccounts();
      break;
    // Registered-repositories list (Settings → Repositories). Only Add/Remove
    // change it — they carry repo_id and are HTTP-driven (there is no agent
    // tool that registers a repo). RepositoryImported is handled above with the
    // artifact events, NOT here: it's a clone into artifacts, not a registered
    // repo.
    case 'RepositoryAdded':
    case 'RepositoryRemoved':
      if (repositories.value.status === 'loaded') void loadRepositories();
      break;
    case 'DeviceRegistered':
    case 'DeviceRenamed':
    case 'DevicePushChanged':
    case 'DeviceDeleted':
      if (devices.value.status === 'loaded') void loadDevices();
      break;
    // Pinned apps are device-scoped. Refresh only when the event targets THIS
    // device — another tab on the same device (or an agent pin/unpin tool) must
    // reflect the change; another device's pins are irrelevant here. The acting
    // tab also receives its own event and reloads (idempotent confirm of the
    // optimistic update). `pinnedApps` is hydrated from localStorage at init so
    // it's effectively always `loaded`; no status gate needed.
    case 'PinnedAppPinned':
    case 'PinnedAppUnpinned':
      if ((data.device_id as string | undefined) === getDeviceId()) {
        void loadPinnedApps();
      }
      break;
    // Thread title events
    case 'ThreadEvent':
      handleThreadTitleEvent(data);
      break;
  }
}

function handleThreadTitleEvent(data: Record<string, unknown>): void {
  const threadId = data.thread_id as string | undefined;
  const event = data.event as Record<string, unknown> | undefined;
  if (!threadId || !event) return;

  const eventType = event.type as string | undefined;
  if (eventType !== 'ThreadTitleGenerated' && eventType !== 'ThreadTitleRenamed') return;

  const title = event.title as string | undefined;
  if (!title) return;

  patchRecentsMetadata(threadId, 'threads', title);
}

function pruneRecents(id: string, category: string): void {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return;
    const recents: Array<{ id: string; category: string }> = JSON.parse(raw);
    const filtered = recents.filter(r => !(r.id === id && r.category === category));
    if (filtered.length < recents.length) {
      localStorage.setItem(RECENTS_KEY, JSON.stringify(filtered));
    }
  } catch { /* corrupted localStorage */ }
}

function patchRecentsMetadata(id: string, category: string, name: string): void {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return;
    const recents: Array<{ id: string; category: string; title: string }> = JSON.parse(raw);
    let changed = false;
    for (const r of recents) {
      if (r.id === id && r.category === category && r.title !== name) {
        r.title = name;
        changed = true;
      }
    }
    if (changed) localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
  } catch { /* corrupted localStorage */ }
}

function pruneNavStack(entity: string, id: string): void {
  try {
    const raw = localStorage.getItem(NAV_KEY);
    if (!raw) return;
    const { stack, cursor }: { stack: Array<Record<string, unknown>>; cursor: number } = JSON.parse(raw);
    if (!Array.isArray(stack)) return;

    let removed = 0;
    const filtered = stack.filter((entry, i) => {
      if (isNavEntryStale(entry, entity, id)) {
        if (i <= cursor) removed++;
        return false;
      }
      return true;
    });

    if (filtered.length < stack.length) {
      const newCursor = Math.max(0, Math.min(cursor - removed, filtered.length - 1));
      localStorage.setItem(NAV_KEY, JSON.stringify({ stack: filtered, cursor: newCursor }));
    }
  } catch { /* corrupted localStorage */ }
}

function isNavEntryStale(entry: Record<string, unknown>, entity: string, id: string): boolean {
  const overlay = entry.overlay as Record<string, unknown> | null;
  if (!overlay) return false;

  switch (entity) {
    case 'app':
      if (overlay.type === 'app-ui') {
        const app = overlay.app as Record<string, unknown> | undefined;
        return app?.id === id;
      }
      if (overlay.type === 'form') {
        const form = overlay.form as Record<string, unknown> | undefined;
        return form?.type === 'app-edit' && form?.appId === id;
      }
      return false;
    case 'trigger':
      if (overlay.type === 'form') {
        const form = overlay.form as Record<string, unknown> | undefined;
        return form?.type === 'trigger' && form?.triggerId === id;
      }
      return false;
    default:
      return false;
  }
}

function closeOverlayIfStale(entity: string, id: string): void {
  const overlay = panelOverlay.value;
  if (!overlay) return;

  switch (entity) {
    case 'app':
      if (overlay.type === 'app-ui' && overlay.app.id === id) {
        panelOverlay.value = null;
        localStorage.removeItem('app-window-open');
      }
      break;
    case 'trigger':
      if (overlay.type === 'form' && overlay.form.type === 'trigger' && 'triggerId' in overlay.form && overlay.form.triggerId === id) {
        panelOverlay.value = null;
      }
      break;
  }
}
