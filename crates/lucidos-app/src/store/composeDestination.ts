/** Compose destination — the compose view's single "who/where" pick for a new
 *  thread (see `system-knowhow/glossary.md` § Compose destination): either the
 *  Lucidos Agent (default — a chat thread) or a coding target (the Lucidos
 *  source, an app, or a registered repository) which spawns a coding-agent
 *  thread of the matching flavor.
 *
 *  This module is the pure half — the type, the option-value codec, and the
 *  caption copy. The mutating `applyDestination` action lives in
 *  `store/actions/compose.ts` with its sibling compose mutations.
 *
 *  `ComposeDestination` is a PRESENTATION-layer union over two existing pieces
 *  of state — it deliberately does not replace their storage:
 *  - the channel pick (`inputMode` signal / per-draft `draft.mode`, wire value
 *    `'lucidos' | 'claude_code'`) — per-thread and cross-device-synced while
 *    composing;
 *  - the coding target (a `Scope`) — resolved PER-DRAFT via
 *    `resolveScope(threadId)` (this draft's `composeSelections` override, or the
 *    global `selectedScope` default) and bound onto the thread's meta at send
 *    time (`sendCompose`). Callers pass the resolved value in; these helpers
 *    never read the global directly.
 *  The picker presents the pair as one control. (The channel pick is
 *  cross-device-synced; the per-draft scope override is device-local — a
 *  pre-existing asymmetry kept as-is.)
 */

import type { Scope } from './store';
import type { ComposeMode } from './actions/compose';
import type { ComposeChannelMode } from './thread-events';

export type ComposeDestination =
  | { kind: 'lucidos-agent' }
  | { kind: 'coding'; scope: Scope };

/** Display name of the Lucidos source as a repository — the registered repo is
 *  named "Lucidos", so a Lucidos-source coding-agent thread shows this string
 *  as its `cc_repo_name` chip (see `repo_name_expr` in the engine). Also the
 *  predicate the destination picker uses to exclude the Lucidos source from the
 *  external-repo list. */
export const LUCIDOS_SOURCE_REPO_NAME = 'Lucidos';

/** Sentinel option value for the "＋ Register a repository…" action row.
 *  NOT part of the destination encoding — callers must check for it BEFORE
 *  `parseOptionValue` (which would fall it through to the Lucidos Agent
 *  default) and route it to Settings → Coding Agents instead. */
export const REGISTER_REPO_OPTION_VALUE = '__register-repo';

/** The Lucidos Agent's consequence blurb — single source for the picker
 *  option description and the caption (which adds the period). */
export const LUCIDOS_AGENT_BLURB = 'Chat, research, and create apps & triggers — can hand off to a coding agent';

/** Derive the current destination from the channel + scope state. The mode
 *  side comes from `effectiveSendMode` (caller resolves it — composing drafts
 *  read `draft.mode`, the bare compose view reads the global `inputMode`). */
export function destinationFromState(mode: ComposeMode, scope: Scope): ComposeDestination {
  return mode === 'claude_code' ? { kind: 'coding', scope } : { kind: 'lucidos-agent' };
}

/** Encode a destination as a Dropdown option value. The Dropdown round-trips
 *  plain strings, so the discriminated union is flattened to:
 *  `agent` | `code:lucidos` | `repo:<uuid>` | `app:<id>`. */
export function destinationToOptionValue(d: ComposeDestination): string {
  if (d.kind === 'lucidos-agent') return 'agent';
  switch (d.scope.kind) {
    case 'lucidos': return 'code:lucidos';
    case 'external': return `repo:${d.scope.repoId}`;
    case 'app': return `app:${d.scope.appId}`;
  }
}

/** Inverse of `destinationToOptionValue`. Unknown values fall back to the
 *  Lucidos Agent — the safe default. Action sentinels (see
 *  `REGISTER_REPO_OPTION_VALUE`) are the caller's job to intercept first. */
export function parseOptionValue(v: string): ComposeDestination {
  if (v === 'code:lucidos') return { kind: 'coding', scope: { kind: 'lucidos' } };
  if (v.startsWith('repo:')) return { kind: 'coding', scope: { kind: 'external', repoId: v.slice(5) } };
  if (v.startsWith('app:')) return { kind: 'coding', scope: { kind: 'app', appId: v.slice(4) } };
  return { kind: 'lucidos-agent' };
}

/** The REPOSITORY a coding scope names, as a display string: the Lucidos source
 *  is its registered repo name, an external repo is looked up in the registry,
 *  an app has no repository at all (its thread is named by its folder, see
 *  `composeDraftContextName`).
 *
 *  Two callers, and they have to agree or the row's chip blinks: the draft row
 *  paints this while composing, and `sendCompose` binds it onto the promoted
 *  thread's `meta.repoName` so the started row paints the same string from the
 *  first frame. The bound value is a stand-in for the engine's `cc_repo_name`,
 *  which arrives a round trip later and then wins (both `upsertThread` and
 *  `applyAggregateToMeta` overwrite `repoName` when the server sends one), so a
 *  repo renamed since the constant was written self-corrects rather than
 *  sticking.
 *
 *  Undefined for an external repo until the registry loads. Both lists are
 *  eager-loaded at startup, so that is a cold-start gap only. */
export function scopeRepoName(
  scope: Scope,
  repos: readonly { id: string; name: string }[],
): string | undefined {
  switch (scope.kind) {
    case 'lucidos': return LUCIDOS_SOURCE_REPO_NAME;
    case 'external': return repos.find(r => r.id === scope.repoId)?.name || undefined;
    case 'app': return undefined;
  }
}

/** The repo/app context-name chip for a compose draft, mirroring
 *  `threadContextName` (which reads a started thread's bound `meta`) but taking
 *  the draft's resolved compose `Scope` (per-draft `resolveScope`, passed in by
 *  the caller) — a draft hasn't bound its meta yet (see `sendCompose`). Returns
 *  undefined for a chat draft: a Lucidos thread carries no context chip, same as
 *  a started chat thread.
 *
 *  Resolution matches what started threads show: an app → the app id (started
 *  app threads chip the id via `appIdFromFolder`, not the name), anything else
 *  → the repository name (see `scopeRepoName`). */
export function composeDraftContextName(
  mode: ComposeChannelMode,
  scope: Scope,
  repos: readonly { id: string; name: string }[],
): string | undefined {
  if (mode !== 'claude_code') return undefined;
  if (scope.kind === 'app') return scope.appId;
  return scopeRepoName(scope, repos);
}

/** One-line consequence caption for the current destination — the compose
 *  view renders it under the picker. Names are resolved by the caller (it
 *  already holds the repos/apps Loadables). */
export function destinationCaption(d: ComposeDestination, names: { appName?: string; repoName?: string } = {}): string {
  if (d.kind === 'lucidos-agent') {
    return `${LUCIDOS_AGENT_BLURB}.`;
  }
  switch (d.scope.kind) {
    case 'lucidos':
      return 'Edits the Lucidos source in an isolated worktree — you review & Apply the change.';
    case 'app':
      return `Edits the ${names.appName ?? d.scope.appId} app in an isolated worktree — you review & Apply the change.`;
    case 'external':
      // External-repo coding-agent threads skip the change-proposal flow —
      // there is no Apply here, the diff is reviewed from the thread.
      return `Edits ${names.repoName ?? 'the repository'} in an isolated worktree — review the diff from the thread.`;
  }
}
