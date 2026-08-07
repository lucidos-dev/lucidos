import { useRef } from 'preact/hooks';
import type { CodingAgent } from '../../api/types';
import { repositories, appsList, enginePackaged, settingsScrollTarget } from '../../store/store';
import { resolveScope, resolveCodingAgent } from '../../store/composeSelections';
import { loadApps } from '../../store/actions/apps';
import { loadRepositories } from '../../store/actions/chat';
import { applyDestination, updateComposeSelection, type ComposeMode } from '../../store/actions/compose';
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
  /** Anchor to scroll to once the subview has rendered. Injected rather than
   *  writing `settingsScrollTarget` inline so the jump stays assertable. */
  scrollToSetting: (anchor: string) => void;
};

type CodingAgentSelectionDeps = {
  patchSelection: typeof updateComposeSelection;
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
    // Action row, not a destination: land on Settings → Coding Agents, scrolled
    // to its Repositories section (repositories share that page with the
    // coding-agent binaries they run under). One call: openSettingsSubview
    // activates the settings menu item too, which is what ContentPane keys off
    // to render SettingsView at all.
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
    patchSelection: updateComposeSelection,
    focusPrompt: focusPromptNow,
  },
): void {
  // Per-draft when a draft is focused (persisted via the debounced compose PUT);
  // PENDING when none exists yet (`updateComposeSelection(null, …)` routes to the
  // pending slot, transferred onto the draft at creation). NEVER the account
  // default — editing a draft (or the fresh compose view) must not write
  // `coding_agent_default` or leak to other drafts. `coding_agent_default` is a
  // fixed seed (from the pref, default Claude Code) — no UI to change it.
  deps.patchSelection(threadId, { codingAgent: value as CodingAgent });
  deps.focusPrompt();
}

type ListRetryDeps = { loadRepos: () => void; loadAppsList: () => void };

/** Retry a destination list whose load FAILED, on the user's gesture of opening
 *  the picker. Nothing else refetches a failed list: the render-path kick-off
 *  below only fires on `not-loaded`, and the SSE refresh only re-fires a list
 *  that is already `loaded` — so one transient failure (a browser-cancelled
 *  fetch, a deadline that fired while the engine was still booting) left the
 *  picker showing a red "Failed to load repositories" row until the user
 *  mutated the repository list by hand. Both loaders single-flight, so at most
 *  one fetch per open, and only for a list that actually failed. */
export function retryFailedDestinationLists(
  deps: ListRetryDeps = { loadRepos: loadRepositories, loadAppsList: loadApps },
): void {
  if (repositories.value.status === 'failed') deps.loadRepos();
  if (appsList.value.status === 'failed') deps.loadAppsList();
}

/** The compose destination picker — its consequence caption,
 *  and the hand-off hint. Lives in its own component so the signal
 *  subscriptions (repositories, appsList, selectedScope, selectedCodingAgent,
 *  preferences via the hint check) re-render this small subtree instead of
 *  the whole PromptInput on unrelated SSE refreshes. */
export function ComposeDestinationRow({ threadId, toggleMode, fading }: {
  threadId: string | null;
  toggleMode: ComposeMode;
  fading: boolean;
}) {
  const reposLoadable = repositories.value;
  const appsLoadable = appsList.value;
  // Idempotent render-path kick-off: each loader single-flights (skips while
  // already loading), so the next render no longer sees `not-loaded` and
  // exactly one fetch fires per source. Fires in every destination state (not
  // just coding) — a restored coding target needs the lists to render its
  // trigger label.
  if (reposLoadable.status === 'not-loaded') void loadRepositories();
  if (appsLoadable.status === 'not-loaded') void loadApps();
  const listsPending = reposLoadable.status === 'loading' || reposLoadable.status === 'not-loaded'
    || appsLoadable.status === 'loading' || appsLoadable.status === 'not-loaded';
  const repos = reposLoadable.status === 'loaded' ? reposLoadable.data : [];
  // External repos = registered repos minus the Lucidos-source row,
  // which is the implicit default and gets the 'Lucidos source' option.
  const externalRepos = repos.filter(r => r.name !== LUCIDOS_SOURCE_REPO_NAME);
  const apps = appsLoadable.status === 'loaded' ? appsLoadable.data : [];

  // Freeze the rendered destination during the fade-out window: the row stays
  // mounted (pointer-events: none) while a newly-focused ACTIVE thread's
  // channel would otherwise recompute `dest` mid-fade and visibly swap the
  // label/chip/caption of a row that's on its way out.
  const lastDestRef = useRef<ComposeDestination>({ kind: 'lucidos-agent' });
  // Per-draft target: resolve THIS draft's scope override (falls back to the
  // global default), so switching one draft's target never changes another's.
  let dest = destinationFromState(toggleMode, resolveScope(threadId));
  if (fading) dest = lastDestRef.current;
  else lastDestRef.current = dest;

  // "Lucidos source" is gated on a dev build: a packaged install has no source
  // checkout, so the option would 500 on send (see composeDestinationOptions).
  const options = buildDestinationOptions({
    dest,
    apps,
    externalRepos,
    packaged: enginePackaged.value,
    listsPending,
    reposError: reposLoadable.status === 'failed' ? reposLoadable.error : null,
    appsError: appsLoadable.status === 'failed' ? appsLoadable.error : null,
  });

  const codingScope = dest.kind === 'coding' ? dest.scope : null;
  const scopeAppId = codingScope?.kind === 'app' ? codingScope.appId : null;
  const scopeRepoId = codingScope?.kind === 'external' ? codingScope.repoId : null;
  const appName = scopeAppId ? apps.find(a => a.id === scopeAppId)?.name : undefined;
  const repoName = scopeRepoId ? repos.find(r => r.id === scopeRepoId)?.name : undefined;

  const value = destinationToOptionValue(dest);

  return (
    <>
      <div class="compose-destination-caption">{destinationCaption(dest, { appName, repoName })}</div>
      <div class="compose-destination-row">
        <Dropdown
          options={options}
          value={value}
          // Shown while the lists are still loading and the restored
          // target's label can't be resolved yet.
          placeholder="…"
          onChange={(v) => {
            handleComposeDestinationSelection(threadId, v);
          }}
          class="compose-destination-picker"
          // Opening the picker retries a list that failed to load — see
          // retryFailedDestinationLists.
          onOpen={retryFailedDestinationLists}
          restoreFocusOnSelect={false}
          // The trigger already shows the current destination; the last-used
          // target is irrelevant when picking a fresh one, so don't mark it in
          // the list (it would only compete with the arrow-key focus row).
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
