
import { actionBlocker, ownBlocker, OWN_BLOCKER_PRIORITY, type ArchiveState, type Blocker, type OwnBlocker } from '../../generated/thread-lifecycle';
import { threadChangeWork, type ThreadState } from '../thread-events';
import { effectiveThreadStatus, getThreadDisplaySection, threadMap } from '../store';
import { BLOCKER_REASON, blockedBySubThreads } from './blockerCopy';
import { collectThreadFamily } from './threadFamily';

export interface BlockingSubThread {
  thread: ThreadState;
  blocker: OwnBlocker;
}

export interface ThreadBlocker {
  blocker: Blocker;
  subThreads: BlockingSubThread[];
}

function threadOwnBlocker(thread: ThreadState, archiveState: ArchiveState): OwnBlocker | null {
  const meta = thread.meta;
  const codingAgent = meta.channel === 'claude_code';
  return ownBlocker(
    codingAgent ? 'claude_code' : 'chat',
    effectiveThreadStatus(thread),
    archiveState,
    threadChangeWork(meta),
    codingAgent && (meta.codingAgentKind === 'external' || meta.codingAgentIsExternalRepo),
  );
}

export function threadBlocker(threadId: string): ThreadBlocker {
  const thread = threadMap.value.get(threadId);
  if (!thread) return { blocker: 'none', subThreads: [] };
  const subThreads: BlockingSubThread[] = [];
  for (const id of collectThreadFamily(threadId)) {
    const member = id === threadId ? undefined : threadMap.value.get(id);
    if (!member) continue;
    const own = threadOwnBlocker(member, member.meta.section);
    if (own) subThreads.push({ thread: member, blocker: own });
  }
  subThreads.sort((a, b) => OWN_BLOCKER_PRIORITY.indexOf(a.blocker) - OWN_BLOCKER_PRIORITY.indexOf(b.blocker));
  const blocker = actionBlocker(threadOwnBlocker(thread, 'inbox'), thread.meta.home === true, subThreads[0]?.blocker ?? null);
  return { blocker, subThreads: blocker.startsWith('descendant_') ? subThreads : [] };
}

export type ExitState = 'enabled' | 'blocked' | null;

export interface ExitItems {
  archive: ExitState;
  unarchive: boolean;
  delete: ExitState;
  reason: string | null;
  subThreads: BlockingSubThread[];
}

export function exitItems(threadId: string): ExitItems {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.state === 'composing') {
    return { archive: null, unarchive: false, delete: null, reason: null, subThreads: [] };
  }
  const { blocker, subThreads } = threadBlocker(threadId);
  const state: ExitState = blocker === 'none' ? 'enabled' : 'blocked';
  const inArchive = getThreadDisplaySection(thread) === 'archive';
  return {
    archive: inArchive ? null : state,
    unarchive: inArchive,
    delete: state,
    reason: blocker === 'none' ? null : subThreads.length > 0 ? blockedBySubThreads(subThreads.length) : BLOCKER_REASON[blocker],
    subThreads,
  };
}
