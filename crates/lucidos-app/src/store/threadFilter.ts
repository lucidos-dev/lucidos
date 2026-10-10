import { ALL_CHANNELS, CODING_AGENT_CHANNEL, type ThreadChannel } from './store';
import type { ThreadState } from './thread-events';
import { appIdFromFolder } from '../utils/appIdFromFolder';

/** Channels the drawer filter knows about. A thread on an unrecognized channel
 *  passes the filter unconditionally (so a future channel isn't silently
 *  hidden before the dropdown learns about it). */
const VALID_CHANNELS: ReadonlySet<string> = new Set<string>(ALL_CHANNELS);

/** Test one thread against the active channel/trigger/repo/app selections.
 *  Trigger-id sub-selection only applies on `trigger` threads; repo-id and
 *  app-id only on the coding-agent channel. Under that channel, repo and app selections
 *  are unioned — a thread passes if its repoId is selected OR (it's an app coding-agent
 *  thread and) its derived app id is selected. Both selection sets empty
 *  means "all coding-agent threads".
 *
 *  Shared by the drawer's display filter (`ThreadDrawer`) and the
 *  infinite-scroll cursor (`loadOlderThreads`) so the two never drift on what
 *  "matches the active filter" means. */
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
