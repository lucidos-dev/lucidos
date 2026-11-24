import { appsList } from '../../store/store';
import { appIdFromFolder } from '../../utils/appIdFromFolder';

export interface ThreadContextFields {
  channel: string;
  triggerName?: string | null;
  repoName?: string | null;
  codingAgentKind?: 'lucidos' | 'app' | 'external' | null;
  codingAgentFolder?: string | null;
}

export function threadContextName(f: ThreadContextFields): string | undefined {
  if (f.channel === 'trigger') return f.triggerName || undefined;
  if (f.channel === 'claude_code') {
    if (f.codingAgentKind !== 'app') return f.repoName || undefined;
    const id = appIdFromFolder(f.codingAgentFolder);
    const apps = appsList.value;
    return (apps.status === 'loaded' ? apps.data.find(app => app.id === id)?.name : undefined)
      || id || undefined;
  }
  return undefined;
}
