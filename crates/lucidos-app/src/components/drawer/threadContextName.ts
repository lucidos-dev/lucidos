import { appIdFromFolder } from '../../utils/appIdFromFolder';

/** Minimal shape needed to name a thread's context. `ThreadMeta` is structurally
 *  assignable; search-result rows (snake-case) map their fields onto it. */
export interface ThreadContextFields {
  channel: string;
  triggerName?: string | null;
  repoName?: string | null;
  codingAgentKind?: 'lucidos' | 'app' | 'external' | null;
  codingAgentFolder?: string | null;
}

/** The specific context NAME shown as a chip in the thread row, alongside the
 *  channel/type tag: the repo name (coding-agent), the app id (app thread), or
 *  the trigger name. Undefined for plain chat — the "Chat" type tag already says
 *  everything, and there's no name to add. */
export function threadContextName(f: ThreadContextFields): string | undefined {
  if (f.channel === 'trigger') return f.triggerName || undefined;
  if (f.channel === 'claude_code') {
    return f.codingAgentKind === 'app'
      ? appIdFromFolder(f.codingAgentFolder) ?? undefined
      : f.repoName || undefined;
  }
  return undefined;
}
