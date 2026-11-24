import { useRef } from 'preact/hooks';
import type { CodingAgent } from '../../api/types';
import { repositories, appsList, enginePackaged, settingsScrollTarget } from '../../store/store';
import { resolveUser, resolveScope, resolveCodingAgent } from '../../store/composeSelections';
import { mentionableUsers } from '../../store/actions/users';
import { loadApps } from '../../store/actions/apps';
import { loadRepositories } from '../../store/actions/chat';
import { applyDestination, selectComposeCodingAgent, type ComposeMode } from '../../store/actions/compose';
import { openSettingsSubview } from '../../store/actions/menu';
import {
  destinationFromState,
  destinationToOptionValue,
  parseOptionValue,
  destinationCaption,
  LUCIDOS_SOURCE_REPO_NAME,
  REGISTER_REPO_OPTION_VALUE,
  type ComposeDestination,
} from '../../store/composeDestination';
import { buildDestinationOptions } from './composeDestinationOptions';
import { Dropdown } from '../shared/Dropdown';
import { focusPromptNow } from './promptFocus';

type DestinationSelectionDeps = {
  apply: typeof applyDestination;
  focusPrompt: () => void;
  openSettingsSubview: typeof openSettingsSubview;
  scrollToSetting: (anchor: string) => void;
};

type CodingAgentSelectionDeps = {
  selectAgent: typeof selectComposeCodingAgent;
  focusPrompt: () => void;
};

export function handleComposeDestinationSelection(
  threadId: string | null,
  value: string,
  deps: DestinationSelectionDeps = {
    apply: applyDestination,
    focusPrompt: focusPromptNow,
    openSettingsSubview,
    scrollToSetting: (anchor) => { settingsScrollTarget.value = anchor; },
  },
): void {
  if (value === REGISTER_REPO_OPTION_VALUE) {
    deps.openSettingsSubview('coding-agents');
    deps.scrollToSetting('coding-agents:repositories');
    return;
  }
  const destination = parseOptionValue(value);
  deps.apply(threadId, destination);
  deps.focusPrompt();
}

export function handleComposeCodingAgentSelection(
  threadId: string | null,
  value: string,
  deps: CodingAgentSelectionDeps = {
    selectAgent: selectComposeCodingAgent,
    focusPrompt: focusPromptNow,
  },
): void {
  void deps.selectAgent(threadId, value as CodingAgent);
  deps.focusPrompt();
}

type ListRetryDeps = { loadRepos: () => void; loadAppsList: () => void };

export function retryFailedDestinationLists(
  deps: ListRetryDeps = { loadRepos: loadRepositories, loadAppsList: loadApps },
): void {
  if (repositories.value.status === 'failed') deps.loadRepos();
  if (appsList.value.status === 'failed') deps.loadAppsList();
}

export function ComposeDestinationRow({ threadId, toggleMode, fading }: {
  threadId: string | null;
  toggleMode: ComposeMode;
  fading: boolean;
}) {
  const reposLoadable = repositories.value;
  const appsLoadable = appsList.value;
  if (reposLoadable.status === 'not-loaded') void loadRepositories();
  if (appsLoadable.status === 'not-loaded') void loadApps();
  const listsPending = reposLoadable.status === 'loading' || reposLoadable.status === 'not-loaded'
    || appsLoadable.status === 'loading' || appsLoadable.status === 'not-loaded';
  const repos = reposLoadable.status === 'loaded' ? reposLoadable.data : [];
  const externalRepos = repos.filter(r => r.name !== LUCIDOS_SOURCE_REPO_NAME);
  const apps = appsLoadable.status === 'loaded' ? appsLoadable.data : [];

  const lastDestRef = useRef<ComposeDestination>({ kind: 'lucidos-agent' });
  const users = mentionableUsers.value;
  let dest = destinationFromState(toggleMode, resolveScope(threadId), resolveUser(threadId));
  if (fading) dest = lastDestRef.current;
  else lastDestRef.current = dest;

  const options = buildDestinationOptions({
    dest,
    apps,
    externalRepos,
    packaged: enginePackaged.value === true,
    listsPending,
    reposError: reposLoadable.status === 'failed' ? reposLoadable.error : null,
    appsError: appsLoadable.status === 'failed' ? appsLoadable.error : null,
    users,
  });

  const codingScope = dest.kind === 'coding' ? dest.scope : null;
  const scopeAppId = codingScope?.kind === 'app' ? codingScope.appId : null;
  const scopeRepoId = codingScope?.kind === 'external' ? codingScope.repoId : null;
  const appName = scopeAppId ? apps.find(a => a.id === scopeAppId)?.name : undefined;
  const repoName = scopeRepoId ? repos.find(r => r.id === scopeRepoId)?.name : undefined;
  const userName = dest.kind === 'user' ? users.find(p => p.id === dest.userId)?.name : undefined;

  const value = destinationToOptionValue(dest);

  return (
    <>
      <div class="compose-destination-caption">{destinationCaption(dest, { appName, repoName, userName })}</div>
      <div class="compose-destination-row">
        <Dropdown
          options={options}
          value={value}
          placeholder="…"
          onChange={(v) => {
            handleComposeDestinationSelection(threadId, v);
          }}
          class="compose-destination-picker"
          onOpen={retryFailedDestinationLists}
          restoreFocusOnSelect={false}
          markCurrent={false}
        />
        {dest.kind === 'coding' && (
          <Dropdown
            options={[
              { value: 'claude-code', label: 'Claude Code' },
              { value: 'codex', label: 'Codex' },
            ]}
            value={resolveCodingAgent(threadId)}
            onChange={(v) => { handleComposeCodingAgentSelection(threadId, v); }}
            class="compose-coding-agent-chip"
            restoreFocusOnSelect={false}
            markCurrent={false}
          />
        )}
      </div>
    </>
  );
}
