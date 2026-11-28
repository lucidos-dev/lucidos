import { ALL_CHANNELS, CODING_AGENT_CHANNEL, type ThreadChannel } from './store';
import type { ThreadState } from './thread-events';
import { appIdFromFolder } from '../utils/appIdFromFolder';

const VALID_CHANNELS: ReadonlySet<string> = new Set<string>(ALL_CHANNELS);

export function threadPassesChannelFilter(
    t: ThreadState,
    channelFilter: ReadonlySet<ThreadChannel>,
    triggerSelection: ReadonlySet<string>,
    repoSelection: ReadonlySet<string>,
    appSelection: ReadonlySet<string>,
): boolean {
    const ch = t.meta.channel;
    const channelOk = channelFilter.has(ch as ThreadChannel) || !VALID_CHANNELS.has(ch);
    if (!channelOk) return false;
    if (ch === 'trigger' && triggerSelection.size > 0) {
        return t.meta.triggerId != null && triggerSelection.has(t.meta.triggerId);
    }
    if (ch === CODING_AGENT_CHANNEL && (repoSelection.size > 0 || appSelection.size > 0)) {
        const repoOk = t.meta.repoId != null && repoSelection.has(t.meta.repoId);
        const appId = t.meta.codingAgentKind === 'app'
            ? appIdFromFolder(t.meta.codingAgentFolder)
            : null;
        const appOk = appId != null && appSelection.has(appId);
        return repoOk || appOk;
    }
    return true;
}
