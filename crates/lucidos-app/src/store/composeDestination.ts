
import type { Scope } from './store';
import type { ComposeMode } from './actions/compose';
import type { ComposeChannelMode, ThreadMeta } from './thread-events';
import { appIdFromFolder } from '../utils/appIdFromFolder';

export type ComposeDestination =
  | { kind: 'lucidos-agent' }
  | { kind: 'coding'; scope: Scope }
  | { kind: 'user'; userId: string };

export const LUCIDOS_SOURCE_REPO_NAME = 'Lucidos';

export const REGISTER_REPO_OPTION_VALUE = '__register-repo';

export const LUCIDOS_AGENT_BLURB = 'Chat, research, build and refine apps & triggers, and hand off to a coding agent';

export function destinationFromState(mode: ComposeMode, scope: Scope, user: string | null = null): ComposeDestination {
  if (mode === 'claude_code') return { kind: 'coding', scope };
  return user ? { kind: 'user', userId: user } : { kind: 'lucidos-agent' };
}

export function destinationToOptionValue(d: ComposeDestination): string {
  if (d.kind === 'lucidos-agent') return 'agent';
  if (d.kind === 'user') return `user:${d.userId}`;
  switch (d.scope.kind) {
    case 'lucidos': return 'code:lucidos';
    case 'external': return `repo:${d.scope.repoId}`;
    case 'app': return `app:${d.scope.appId}`;
  }
}

export function parseOptionValue(v: string): ComposeDestination {
  if (v === 'code:lucidos') return { kind: 'coding', scope: { kind: 'lucidos' } };
  if (v.startsWith('repo:')) return { kind: 'coding', scope: { kind: 'external', repoId: v.slice(5) } };
  if (v.startsWith('app:')) return { kind: 'coding', scope: { kind: 'app', appId: v.slice(4) } };
  if (v.startsWith('user:')) return { kind: 'user', userId: v.slice('user:'.length) };
  return { kind: 'lucidos-agent' };
}

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

export function threadScope(
  meta: Pick<ThreadMeta, 'codingAgentKind' | 'repoId' | 'codingAgentFolder'>,
): Scope | null {
  switch (meta.codingAgentKind ?? 'lucidos') {
    case 'lucidos': return { kind: 'lucidos' };
    case 'external': return meta.repoId ? { kind: 'external', repoId: meta.repoId } : null;
    case 'app': {
      const appId = appIdFromFolder(meta.codingAgentFolder);
      return appId ? { kind: 'app', appId } : null;
    }
  }
}

export function composeDraftContextName(
  mode: ComposeChannelMode,
  scope: Scope,
  repos: readonly { id: string; name: string }[],
): string | undefined {
  if (mode !== 'claude_code') return undefined;
  if (scope.kind === 'app') return scope.appId;
  return scopeRepoName(scope, repos);
}

export function destinationCaption(d: ComposeDestination, names: { appName?: string; repoName?: string; userName?: string } = {}): string {
  if (d.kind === 'lucidos-agent') {
    return `${LUCIDOS_AGENT_BLURB}.`;
  }
  if (d.kind === 'user') {
    return `A thread with ${names.userName ?? 'a teammate'}. The agent stays paused until someone mentions @agent.`;
  }
  switch (d.scope.kind) {
    case 'lucidos':
      return 'Edits the Lucidos source in an isolated worktree — you review & Apply the change.';
    case 'app':
      return `Edits the ${names.appName ?? d.scope.appId} app in an isolated worktree — you review & Apply the change.`;
    case 'external':
      return `Edits ${names.repoName ?? 'the repository'} in an isolated worktree — review the diff from the thread.`;
  }
}
