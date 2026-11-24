import { CODING_AGENT_TARGETS } from '@lucidos/sdk';
import { appsList, focusedThreadId, repositories, showToast } from '../store';
import type { Repository, Scope } from '../store';
import type { CodingAgent } from '../../api/types';
import { LUCIDOS_SOURCE_REPO_NAME } from '../composeDestination';
import { resolveScope } from '../composeSelections';
import { applyDestination, updateComposeSelection } from './compose';
import { refreshApps } from './apps';
import { matchRepository, rereadAndMatch } from './repoFileLink';

export interface NewChatPicks {
  coding_agent?: string;
  folder?: string;
}

const APP_FOLDER_PREFIX = 'data/apps/';

type FolderResolution = { scope: Scope } | { problem: string };

const isCodingAgent = (value: string): value is CodingAgent =>
  (CODING_AGENT_TARGETS as readonly string[]).includes(value);

const repoScope = (repo: Repository): Scope =>
  repo.name === LUCIDOS_SOURCE_REPO_NAME ? { kind: 'lucidos' } : { kind: 'external', repoId: repo.id };

async function resolveRepoFolder(folder: string): Promise<FolderResolution> {
  const match = await rereadAndMatch(folder);
  switch (match.kind) {
    case 'found': return { scope: repoScope(match.repo) };
    case 'missing': return { problem: 'no registered repository or installed app matches it' };
    case 'ambiguous': return { problem: `${match.count} registered repositories share that name, so name it by id` };
    case 'failed': return { problem: `failed to load repositories: ${match.error}` };
  }
}

async function resolveAppFolder(appId: string): Promise<FolderResolution> {
  await refreshApps();
  const apps = appsList.value;
  if (apps.status !== 'loaded') return { problem: 'apps failed to load' };
  return apps.data.some((a) => a.id === appId)
    ? { scope: { kind: 'app', appId } }
    : { problem: `no installed app has the id "${appId}"` };
}

function resolveFolder(folder: string): FolderResolution | Promise<FolderResolution> {
  if (folder.startsWith(APP_FOLDER_PREFIX)) {
    const appId = folder.slice(APP_FOLDER_PREFIX.length).replace(/\/+$/, '');
    const apps = appsList.value;
    if (apps.status === 'loaded' && apps.data.some((a) => a.id === appId)) {
      return { scope: { kind: 'app', appId } };
    }
    return resolveAppFolder(appId);
  }
  const repos = repositories.value;
  const cached = repos.status === 'loaded' ? matchRepository(repos.data, folder) : null;
  return cached?.kind === 'found' ? { scope: repoScope(cached.repo) } : resolveRepoFolder(folder);
}

function aimDraft(threadId: string, backend: CodingAgent | null, scope: Scope | null): void {
  if (backend === null && scope === null) return;
  applyDestination(threadId, { kind: 'coding', scope: scope ?? resolveScope(threadId) });
  if (backend !== null) updateComposeSelection(threadId, { codingAgent: backend });
}

export function prefillNewChatDestination(
  threadId: string,
  picks: NewChatPicks,
  source?: string,
): Promise<void> | void {
  const agent = picks.coding_agent || undefined;
  const folder = picks.folder || undefined;
  if (agent === undefined && folder === undefined) return;
  const from = source ? ` (requested by ${source})` : '';

  let backend: CodingAgent | null = null;
  if (agent !== undefined) {
    if (isCodingAgent(agent)) {
      backend = agent;
    } else {
      showToast(
        `Couldn't preselect coding agent "${agent}"${from}: use ${CODING_AGENT_TARGETS.map((a) => `"${a}"`).join(' or ')}`,
        'error',
      );
    }
  }
  if (folder === undefined) return aimDraft(threadId, backend, null);

  const settle = (resolution: FolderResolution): void => {
    if ('problem' in resolution) {
      showToast(`Couldn't preselect folder "${folder}"${from}: ${resolution.problem}`, 'error');
    }
    if (focusedThreadId.value !== threadId) return;
    aimDraft(threadId, backend, 'scope' in resolution ? resolution.scope : null);
  };
  const resolution = resolveFolder(folder);
  return resolution instanceof Promise ? resolution.then(settle) : settle(resolution);
}
