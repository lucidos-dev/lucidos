import { API, ApiError, conditionalJson } from './_core';
import type { getPinnedAppUis, listAppsApi } from './apps';
import type { engineVersionStatus, fetchChanges } from './changes';
import type { listModels } from './models';
import type { fetchInstalledPlugins, fetchPluginCatalog } from './pluginMarketplaces';
import type { readerDevices } from './readerDevices';
import type { releaseNotices } from './releaseNotices';
import type {
  getChatLadder, getEmbeddingModelStatus, getLastSuccessfulBackup, getNotifications, getPreferences,
  listDevices,
} from './settings';
import type { getSpendCapHolds } from './spendCapHolds';
import type { getThreadQueue } from './threadQueue';
import type { fetchFilterFacets } from '../threads';
import type { listHistoricalTriggers, listTriggerGroups, listTriggers } from './triggers';
import type { listUsers } from './users';
import type { fetchWebhookIngress, fetchWebhookRefusals } from './webhooks';
import type { Repository } from '../../store/store';

export type SliceResult<Body> =
  | { data: Body }
  | { error: { message: string; http_code: number } };

type RouteBody<Read extends (...args: never[]) => Promise<unknown>> = SliceResult<Awaited<ReturnType<Read>>>;

/** `GET /api/v1/bootstrap`: the shell-wide state, one slice per narrow route.
 *
 *  Hand-written until the wire-types crate generates REST types
 *  (docs/temporary-measures.md § Bootstrap snapshot type is hand-written). */
export interface BootstrapSnapshot {
  preferences: RouteBody<typeof getPreferences>;
  unread_notifications: RouteBody<typeof getNotifications>;
  reader_devices: RouteBody<typeof readerDevices>;
  users: RouteBody<typeof listUsers>;
  pinned_apps: RouteBody<typeof getPinnedAppUis>;
  apps: RouteBody<typeof listAppsApi>;
  chat_models: RouteBody<typeof listModels>;
  intelligence_ladder: RouteBody<typeof getChatLadder>;
  spend_cap_holds: RouteBody<typeof getSpendCapHolds>;
  last_successful_backup: RouteBody<typeof getLastSuccessfulBackup>;
  release_notices: RouteBody<typeof releaseNotices>;
  embedding_model_status: RouteBody<typeof getEmbeddingModelStatus>;
  webhook_ingress: RouteBody<typeof fetchWebhookIngress>;
  webhook_refusals: RouteBody<typeof fetchWebhookRefusals>;
  installed_plugins: RouteBody<typeof fetchInstalledPlugins>;
  plugin_catalog: RouteBody<typeof fetchPluginCatalog>;
  triggers: RouteBody<typeof listTriggers>;
  historical_triggers: RouteBody<typeof listHistoricalTriggers>;
  trigger_groups: RouteBody<typeof listTriggerGroups>;
  thread_queue: RouteBody<typeof getThreadQueue>;
  repositories: SliceResult<Repository[]>;
  filter_facets: RouteBody<typeof fetchFilterFacets>;
  devices: RouteBody<typeof listDevices>;
  changes_state: RouteBody<typeof fetchChanges>;
  engine_version_status: RouteBody<typeof engineVersionStatus>;
}

export type BootstrapSliceKey = keyof BootstrapSnapshot;

export type BootstrapSliceBody<Key extends BootstrapSliceKey> =
  BootstrapSnapshot[Key] extends SliceResult<infer Body> ? Body : never;

export type BootstrapRead =
  | { kind: 'snapshot'; snapshot: BootstrapSnapshot; etag: string | null }
  | { kind: 'not-modified' }
  | { kind: 'route-missing' };

/** Ask for the snapshot, with the ETag of the one already held.
 *
 *  A 404 is an engine older than the route, and the one failure this answers
 *  instead of throwing (docs/temporary-measures.md § Bootstrap 404 fallback). */
export async function fetchBootstrap(etag: string | null): Promise<BootstrapRead> {
  try {
    const read = await conditionalJson<BootstrapSnapshot>(`${API}/bootstrap`, etag);
    return read.modified
      ? { kind: 'snapshot', snapshot: read.body, etag: read.etag }
      : { kind: 'not-modified' };
  } catch (error) {
    if (error instanceof ApiError && error.httpCode === 404) return { kind: 'route-missing' };
    throw error;
  }
}
