import { useRef } from 'preact/hooks';
import type { CodingAgent } from '../../api/types';
import { repositories, selectedScope, selectedCodingAgent, appsList } from '../../store/store';
import { loadApps } from '../../store/actions/apps';
import { loadRepositories } from '../../store/actions/chat';
import { applyDestination, type ComposeMode } from '../../store/actions/compose';
import { switchMenuItem, openSettingsSubview } from '../../store/actions/menu';
import { composeHandoffHintDismissed, dismissComposeHandoffHint, setCodingAgentDefault } from '../../store/actions/preferences';
import {
  destinationFromState,
  destinationToOptionValue,
  parseOptionValue,
  destinationCaption,
  LUCIDOS_AGENT_BLURB,
  REGISTER_REPO_OPTION_VALUE,
  type ComposeDestination,
} from '../../store/composeDestination';
import { Dropdown, type DropdownOption } from '../shared/Dropdown';
import { CloseIcon } from '../shared/icons';

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
  const externalRepos = repos.filter(r => r.name !== 'Lucidos');
  const apps = appsLoadable.status === 'loaded' ? appsLoadable.data : [];

  // Freeze the rendered destination during the fade-out window: the row stays
  // mounted (pointer-events: none) while a newly-focused ACTIVE thread's
  // channel would otherwise recompute `dest` mid-fade and visibly swap the
  // label/chip/caption of a row that's on its way out.
  const lastDestRef = useRef<ComposeDestination>({ kind: 'lucidos-agent' });
  let dest = destinationFromState(toggleMode, selectedScope.value);
  if (fading) dest = lastDestRef.current;
  else lastDestRef.current = dest;

  const options: DropdownOption[] = [
    {
      value: destinationToOptionValue({ kind: 'lucidos-agent' }),
      label: 'Lucidos Agent',
      description: LUCIDOS_AGENT_BLURB,
    },
    { value: '__hdr-coding', label: 'Coding agent on…', disabled: true },
    {
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'lucidos' } }),
      label: 'Lucidos source',
      description: "The Lucidos platform's own code",
    },
  ];
  for (const a of apps) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'app', appId: a.id } }),
      label: `${a.name} · app`,
    });
  }
  for (const r of externalRepos) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'external', repoId: r.id } }),
      label: `${r.name} · repository`,
    });
  }
  // Loading must not look like "no apps / no repos" — the open menu carries
  // an explicit pending row until both lists have settled.
  if (listsPending) {
    options.push({ value: '__loading', label: 'Loading…', disabled: true });
  }
  // Failed must look different from empty — each list gets its own error row.
  if (reposLoadable.status === 'failed') {
    options.push({
      value: '__repos-error',
      label: 'Failed to load repositories',
      description: reposLoadable.error,
      disabled: true,
      danger: true,
    });
  }
  if (appsLoadable.status === 'failed') {
    options.push({
      value: '__apps-error',
      label: 'Failed to load apps',
      description: appsLoadable.error,
      disabled: true,
      danger: true,
    });
  }
  options.push({ value: REGISTER_REPO_OPTION_VALUE, label: '＋ Register a repository…' });

  const codingScope = dest.kind === 'coding' ? dest.scope : null;
  const scopeAppId = codingScope?.kind === 'app' ? codingScope.appId : null;
  const scopeRepoId = codingScope?.kind === 'external' ? codingScope.repoId : null;
  const appName = scopeAppId ? apps.find(a => a.id === scopeAppId)?.name : undefined;
  const repoName = scopeRepoId ? repos.find(r => r.id === scopeRepoId)?.name : undefined;

  // A restored coding target whose app/repo no longer exists would render the
  // loading placeholder forever — surface it as an explicit unavailable row
  // instead once the lists have settled (sending would target a dead folder;
  // the engine rejects it, but the picker shouldn't pretend it's loading).
  const value = destinationToOptionValue(dest);
  if (!listsPending && !options.some(o => o.value === value)) {
    options.push({
      value,
      label: `${scopeAppId ?? scopeRepoId ?? value} · unavailable`,
      disabled: true,
      danger: true,
    });
  }

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
            if (v === REGISTER_REPO_OPTION_VALUE) {
              // Action row, not a destination: land on Settings →
              // Repositories. openSettingsSubview only sets the subview —
              // ContentPane renders SettingsView only when the settings menu
              // item is active, so switch first (same pairing as
              // SearchEverywhere).
              switchMenuItem('settings');
              openSettingsSubview('repositories');
              return;
            }
            applyDestination(threadId, parseOptionValue(v));
          }}
          class="compose-destination-picker"
        />
        {dest.kind === 'coding' && (
          <Dropdown
            options={[
              { value: 'claude-code', label: 'Claude Code' },
              { value: 'codex', label: 'Codex' },
            ]}
            value={selectedCodingAgent.value}
            onChange={(v) => { void setCodingAgentDefault(v as CodingAgent); }}
            class="compose-coding-agent-chip"
          />
        )}
      </div>
      {dest.kind === 'lucidos-agent' && !composeHandoffHintDismissed() && (
        <div class="compose-handoff-hint">
          <span>Not sure? Start with the Lucidos Agent — it can hand off to a coding agent.</span>
          <button
            class="icon-btn compose-handoff-hint-dismiss"
            aria-label="Dismiss hint"
            onClick={() => { void dismissComposeHandoffHint(); }}
          >
            <CloseIcon />
          </button>
        </div>
      )}
    </>
  );
}
