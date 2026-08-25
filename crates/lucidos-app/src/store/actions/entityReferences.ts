/**
 * EntityReferenceManager — independent SSE consumer that keeps entity references
 * (recents, nav stack, pinned apps, active overlay) in sync when entities change.
 *
 * Wired at the SSE dispatch level in thread-sync.ts, NOT as a side-effect of
 * handleThreadEvent or handleGlobalEvent.
 */
import { panelOverlay, appsList, installedPlugins, marketplaceCatalog, triggers, credentials, environmentVariables, chatModels, oauthAccounts, repositories, artifacts, llmConfigured, configuredProviders, mcpServersVersion, webhooksVersion, permissionGrantsVersion } from '../store';
import { checkHealth } from '../../api/client';
import { loadApps } from './apps';
import { loadInstalledPlugins } from './plugins';
import { refreshPluginCatalogAfterMutation } from './plugin-marketplaces';
import { loadChatModels } from './models';
import { loadTriggers, loadHistoricalTriggers } from './triggers';
import { loadThreadQueue } from './threadQueue';
import { loadTriggerGroups } from './triggerGroups';
import { loadArtifacts, invalidateFilePreview } from './artifacts';
import { removePinnedAppLocal, loadPinnedApps } from './pinnedApps';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadOAuthAccounts, handleOAuthAccountConnected } from './oauth';
import { loadRepositories } from './repositoriesLoader';
import { loadDevices, devices, getDeviceId } from './devices';

export const RECENTS_KEY = 'lucidos-search-recents';
export const NAV_KEY = 'lucidos-nav-history';

/** Should an entity event refresh the `artifacts` list?
 *
 *  `not-loaded` means the user never opened a surface that needs it, and
 *  warming a cache nobody asked for is pure network waste. Every other state
 *  means they DID ask, `failed` included.
 *
 *  Admitting `failed` is what stops one bad fetch latching. `loadArtifacts`
 *  writes `toFailed` on error, which discards the loaded list, so a single
 *  timed-out refresh used to leave every later event unable to retry: the gate
 *  wanted `loaded` and nothing could get back there short of a page reload.
 *  Chat links, the Files panel and the preview all went stale together and
 *  stayed that way.
 *
 *  `loading` is excluded so a burst of events cannot stampede parallel fetches
 *  whose completion order decides the winner. */
function artifactsWereAskedFor(): boolean {
  const status = artifacts.value.status;
  return status === 'loaded' || status === 'failed';
}

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

/** Re-fetch the open preview when an `Artifact*` event names the file it shows.
 *
 *  These events carry `artifact_path` relative to `artifacts/`, while the
 *  preview addresses a file the data-relative way, so the prefix goes back on
 *  here. `invalidateFilePreview` ignores every other path. */
function invalidateArtifactPreview(data: Record<string, unknown>): void {
  const artifactPath = data.artifact_path as string | undefined;
  if (artifactPath) invalidateFilePreview(`artifacts/${artifactPath}`);
}

/** The plugin id, wherever the frame carries it. `PluginInstalled` nests it in
 *  the manifest; every other `Plugin*` frame has it at the top level. */
function pluginIdOf(data: Record<string, unknown>): string | undefined {
  const manifest = data.manifest as Record<string, unknown> | undefined;
  return (data.id as string | undefined) ?? (manifest?.id as string | undefined);
}

/** Close a plugin install / uninstall panel whose pending entry somebody else
 *  resolved.
 *
 *  Both panels open from a THREAD event, so every client of that thread shows
 *  one. Once a peer confirms or cancels, the pending entry is gone and Confirm
 *  here can only 404, which `cancelPluginInstallAction` swallows: the panel
 *  vanishes and nothing happened.
 *
 *  **The acting device is exempt by its ACTOR, not by the receipt its confirm
 *  leaves.** The engine emits before it answers the POST, so the frame can
 *  arrive while that panel is still pending. Closing on it would beat
 *  `markPluginInstalled` to the overlay, and the receipt would silently
 *  degrade to a toast. Same test as `handleOAuthAccountConnected`.
 *
 *  A receipt is skipped too. It is already resolved, so there is nothing to
 *  close and a record to lose. */
function closeResolvedPluginPanel(
  eventType: string,
  pluginId: string | undefined,
  actor: { kind?: string; device_id?: string } | null | undefined,
): void {
  if (!pluginId) return;
  if (actor?.kind === 'device' && actor.device_id === getDeviceId()) return;
  const overlay = panelOverlay.value;
  if (overlay?.type !== 'form') return;
  const form = overlay.form;
  const installing = eventType === 'PluginInstalled' || eventType === 'PluginInstallCanceled';
  if (installing) {
    if (form.type !== 'plugin-install' || form.installed) return;
  } else if (form.type !== 'plugin-uninstall' || form.removed) {
    return;
  }
  if (form.request.plugin_id !== pluginId) return;
  panelOverlay.value = null;
}

/** The `actor` a `Plugin*` frame carries, in the shape the device test needs. */
function actorOf(data: Record<string, unknown>): { kind?: string; device_id?: string } | null {
  return (data.actor as { kind?: string; device_id?: string } | null | undefined) ?? null;
}

export function refreshLlmConfigured(): void {
  void probeLlmConfigured();
  setTimeout(() => {
    void probeLlmConfigured();
  }, 600);
}

/** Preference keys that change which providers the engine has installed, and so
 *  need the same `/health` re-probe a credential change gets. Mirrors
 *  `PROVIDER_PREFERENCE_KEYS` in the engine's `llm/provider_build.rs`, which is
 *  the set its config subscriber rebuilds on. Without the probe the model
 *  picker keeps offering a provider the engine has already dropped. */
export const PROVIDER_PREFERENCE_KEYS: ReadonlySet<string> = new Set([
  'opencode_free_enabled',
  'local_base_url',
  'provider_enabled_vertex',
  'provider_enabled_anthropic',
  'provider_enabled_openai',
  'provider_enabled_openrouter',
  'provider_enabled_xai',
  'provider_enabled_local',
]);

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
      // A deleted trigger joins the HISTORICAL registry, which is what the
      // thread filter offers as trigger options. Nothing else reloads that
      // list, so without this the filter keeps offering the live trigger and
      // never offers the historical one until a page reload.
      void loadHistoricalTriggers();
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
    // refresh the same `artifacts` list. The latter three are gated on
    // `artifactsWereAskedFor`, matching the PluginInstalled pattern: don't warm
    // a cache the user hasn't opened. ArtifactImported is unconditional,
    // because the import flow always has a settings panel open.
    //
    // All four also carry `artifact_path`, relative to `artifacts/`. Prefixed
    // back to the data-relative form, that is what tells the open preview
    // whether the file IT shows just changed. The list refresh and the preview
    // invalidation are independent: a preview can be open on a file whose list
    // this device never loaded, so the gate must not swallow it.
    case 'ArtifactImported':
      invalidateArtifactPreview(data);
      void loadArtifacts();
      break;
    case 'ArtifactCreated':
    case 'ArtifactUpdated':
    case 'ArtifactDeleted':
      invalidateArtifactPreview(data);
      if (artifactsWereAskedFor()) void loadArtifacts();
      break;
    // `RepositoryImported` is the `git_clone` tool landing a repo's files under
    // `data/artifacts/imported/<name>/` — a BULK ARTIFACT import, not a change
    // to the registered-repositories list (it carries url/destination/files,
    // no repo_id). So it refreshes the `artifacts` cache, same gating as the
    // Artifact* trio above. (Previously it wrongly reloaded the registered-repo
    // list, which never contains the clone — so an agent-imported repo never
    // appeared anywhere live.)
    case 'RepositoryImported':
      if (artifactsWereAskedFor()) void loadArtifacts();
      break;
    // Data-file mutations via the HTTP `/data/*` API (SDK `lucidos.data.*`,
    // `lucidos` CLI). These are the API-origin AUDIT events; the paired
    // `Artifact*` entity event now fires alongside them, emitted from inside
    // `ArtifactManager`'s write path. This arm used to be the workaround for
    // its absence, and is kept because a data-API write to a non-artifact
    // subtree still only produces a `DataFile*`, and because reloading the
    // same list twice is idempotent. `path` is relative to `data/`
    // (e.g. `artifacts/notes.md`); refresh the artifacts cache when it targets
    // the artifacts subtree. Same gate as the Artifact* trio.
    case 'DataFileWritten':
    case 'DataFileEdited':
    case 'DataFileDeleted': {
      const dataPath = data.path as string | undefined;
      // Already data-relative, so it goes straight to the preview. Note the
      // wider reach than the list refresh below: a `config/` or `knowhow/`
      // write changes no artifact list, but it may well be the file on screen.
      if (dataPath) invalidateFilePreview(dataPath);
      if (dataPath?.startsWith('artifacts/') && artifactsWereAskedFor()) {
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
      closeResolvedPluginPanel(type, pluginIdOf(data), actorOf(data));
      break;
    // A peer resolved the pending entry this device's panel is offering, so
    // its buttons can now only 404. Close it rather than leave a dead one up.
    case 'PluginInstallCanceled':
    case 'PluginUninstallCanceled':
      closeResolvedPluginPanel(type, pluginIdOf(data), actorOf(data));
      break;
    // Marketplace registry mutations. One arm serves BOTH surfaces that list
    // marketplaces, because both render off the single `marketplaceCatalog`
    // signal: the Plugins panel (StoreTab, for its marketplace-name labels and
    // empty-state gate) and Settings → Marketplaces (the list itself).
    // `PluginMarketplaceRegistered` is an upsert event, so this covers a
    // RENAME as much as a first registration. The rename was the reported bug
    // (an agent re-registered a source under a new name mid-chat and the open
    // panel kept the old one until a reload), and gating on "is this new?"
    // would drop exactly that case.
    // The `loaded` gate is not optional here: the catalog scan git-clones every
    // registered marketplace, so refreshing on a device that never opened the
    // panel is the most expensive possible version of warming an unwanted
    // cache. The acting client refreshes anyway after its own POST resolves;
    // that call joins this one's in-flight scan rather than starting a second.
    case 'PluginMarketplaceRegistered':
    case 'PluginMarketplaceRemoved':
      if (marketplaceCatalog.value.status === 'loaded') void refreshPluginCatalogAfterMutation();
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
    // Three settings pages fetch their own data rather than holding it in a
    // store signal, so they subscribe through a version counter instead: they
    // re-read when it moves. See `useVersionedRefresh`. Bumping is free when
    // the page is closed, since nothing reads the counter then.
    case 'McpServerRegistered':
    case 'McpServerUpdated':
    case 'McpServerRemoved':
    case 'McpServerDisabledToolsChanged':
      mcpServersVersion.value++;
      break;
    case 'WebhookCreated':
    case 'WebhookUpdated':
    case 'WebhookDeleted':
      webhooksVersion.value++;
      break;
    // The agent grants a command or tool pattern by writing the allowlist file,
    // which is exactly what the Permissions editors show. Both files bump the
    // one counter; see the signal's own note for why it is not keyed.
    case 'PermissionGrantsChanged':
      permissionGrantsVersion.value++;
      break;
    // Settings → Accounts. Both halves of the pair matter: connecting used to
    // emit nothing at all (the engine wrote the row straight from the OAuth
    // callback), so a disconnect refreshed every client while a connect
    // refreshed none. The engine now emits both from inside
    // `OAuthStore::{connect,delete}`, so the browser tab that ran the flow and
    // every other device converge without a reload. A token refresh
    // deliberately emits nothing: nothing user-visible changed.
    // Connecting does more than refresh the list: on the device that STARTED
    // the flow it also closes the callback page if it's in the in-app browser
    // panel, toasts which account landed, and fronts the window. The user's
    // attention is in a browser at that moment. See `handleOAuthAccountConnected`.
    case 'OAuthAccountConnected':
      handleOAuthAccountConnected(data as {
        provider?: string;
        email?: string | null;
        actor?: { kind?: string; device_id?: string } | null;
      });
      break;
    case 'OAuthAccountDeleted':
      if (oauthAccounts.value.status === 'loaded') void loadOAuthAccounts();
      break;
    // Registered-repositories list (Settings → Coding Agents). Only Add/Remove
    // change it, and both carry repo_id. The engine emits them from INSIDE its
    // single registry write path (`RepositoryStore::{register,unregister}`), not
    // per call site, so every way a repo can appear reaches this arm: the HTTP
    // CRUD, the `manage_repositories` agent tool (the user asking for a repo in
    // chat), and the engine's own startup registration. The tool used to write
    // the row directly and emit nothing, which left an agent-registered repo
    // invisible here until a page reload. RepositoryImported is handled above
    // with the artifact events, NOT here: it's a clone into artifacts, not a
    // registered repo.
    case 'RepositoryAdded':
    case 'RepositoryRemoved':
      if (repositories.value.status === 'loaded') void loadRepositories();
      break;
    case 'DeviceRegistered':
    case 'DeviceRenamed':
    case 'DevicePushChanged':
    case 'DeviceDeleted':
    // A hand-over moves a whole row to a new id, so a Settings page open on
    // another device is showing one that no longer exists.
    case 'DeviceHandedOver':
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
