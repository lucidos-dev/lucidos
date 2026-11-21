/** The *action blocker* for a thread, from what the client has loaded
 *  (ADR 0378). The thread menu draws from it; the engine's refusal decides.
 *
 *  Descendants come from walking the loaded family. When one blocks but is not
 *  loaded, this answers `none`, the menu leaves the item enabled, and the
 *  engine's 409 names the blocker instead.
 */

import { actionBlocker, ownBlocker, OWN_BLOCKER_PRIORITY, type ArchiveState, type Blocker, type OwnBlocker } from '../../generated/thread-lifecycle';
import { threadChangeWork, type ThreadState } from '../thread-events';
import { effectiveThreadStatus, getThreadDisplaySection, threadMap } from '../store';
import { BLOCKER_REASON, blockedBySubThreads } from './blockerCopy';
import { collectThreadFamily } from './threadFamily';

/** A loaded sub-thread that holds Archive and Delete back, and why. */
export interface BlockingSubThread {
  thread: ThreadState;
  blocker: OwnBlocker;
}

export interface ThreadBlocker {
  blocker: Blocker;
  /** Every loaded sub-thread that blocks, strongest blocker first. Empty
   *  unless the blocker is a descendant one. */
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
  // Stable, so sub-threads with the same blocker keep the family's order.
  subThreads.sort((a, b) => OWN_BLOCKER_PRIORITY.indexOf(a.blocker) - OWN_BLOCKER_PRIORITY.indexOf(b.blocker));
  const blocker = actionBlocker(threadOwnBlocker(thread, 'inbox'), thread.meta.home === true, subThreads[0]?.blocker ?? null);
  return { blocker, subThreads: blocker.startsWith('descendant_') ? subThreads : [] };
}

/** How the thread menu draws one exit: offered, shown blocked, or absent. */
export type ExitState = 'enabled' | 'blocked' | null;

export interface ExitItems {
  archive: ExitState;
  /** A thread in the Archive section offers Move to Current where Archive was. */
  unarchive: boolean;
  delete: ExitState;
  /** Why the exits are blocked. For a descendant blocker it introduces the
   *  `subThreads` list. */
  reason: string | null;
  subThreads: BlockingSubThread[];
}

/** The thread menu's Archive and Delete rows. A blocker never removes a row:
 *  only a draft has none, and a thread in the Archive section trades Archive
 *  for Move to Current.
 *
 *  Placement is where the drawer shows the thread, not where it is stored. A
 *  thread stored archived but kept in Current still offers Archive: blocked by
 *  what keeps it there, or enabled when nothing visible does. Pressing it then
 *  makes the engine recount the family, which heals a drifted count. */
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
