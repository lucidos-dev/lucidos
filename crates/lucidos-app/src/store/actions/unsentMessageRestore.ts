/** Bring back the unsent messages a previous page load left: sends whose
 *  outcome it never learned. A reload, often iOS unloading a backgrounded PWA,
 *  would otherwise lose them with no word. Each comes back as a Not sent card
 *  with Retry, where it was sent. An answer typed to a question card comes
 *  back on that card while it still waits. One the engine has settles as
 *  accepted, and one whose thread is gone moves into a fresh draft.
 *
 *  Plan: `docs/plans/2026-10-03-unsent-messages-survive-a-reload.md`. */

import { effect } from '@preact/signals';

import { makeOptimisticThreadState } from '../thread-events';
import { holdPageOwnerLock, liveOwnerIds, ownedByAnotherLiveTab, pageOwnerId } from '../pageOwner';
import { isConnected, showToast, threadListFetched, threadMap } from '../store';
import {
  adoptUnsentMessageRecord,
  forgetUnsentMessageRecord,
  readUnsentMessageStore,
  type UnsentMessageRecord,
} from '../unsentMessageRecords';
import { unsentMessages } from '../unsentMessages';
import { errorDetail } from '../../utils/errorDetail';
import { engineRecordedMessage, showRestoredUnsentMessage } from './chat';
import { composeInFreshDraft, resumeUnsentFirstSend } from './compose';
import { settleAcceptedSend } from './sendSettlement';
import { ensureThreadByIdInMap } from './thread-loading';

/** What a restore pass found, for the one toast that reports it. */
export interface UnsentRestoreOutcome {
  shown: number;
  /** The engine had them after all, so they settled as accepted. */
  delivered: number;
  /** Their thread is gone, so the text moved into a fresh draft. */
  movedToDraft: number;
  /** Records whose thread could not be checked. A later pass retries them. */
  unresolved: number;
  unresolvedReason?: string;
}

/** The toast after a pass that moved or could not check anything, or null. */
export function unsentRestoreMessage(
  outcome: Pick<UnsentRestoreOutcome, 'movedToDraft' | 'unresolved' | 'unresolvedReason'>,
): string | null {
  const parts: string[] = [];
  if (outcome.movedToDraft > 0) {
    parts.push(outcome.movedToDraft === 1
      ? 'An unsent message from before the reload is back in a new draft: its thread no longer exists.'
      : `${outcome.movedToDraft} unsent messages from before the reload are back in new drafts: their threads no longer exist.`);
  }
  if (outcome.unresolved > 0) {
    const head = outcome.unresolved === 1
      ? 'An unsent message from before the reload could not be brought back yet'
      : `${outcome.unresolved} unsent messages from before the reload could not be brought back yet`;
    parts.push(`${head} (${outcome.unresolvedReason ?? 'unknown error'}). Lucidos tries again on reconnect and on the next reload.`);
  }
  return parts.join(' ') || null;
}

type ThreadVerdict = 'live' | 'gone' | 'never-made' | 'made-by-this-send';

/** Ask whether a record's thread exists. The loaded thread list is only a
 *  window, so a thread missing from it is looked up by id. Throws when the
 *  engine could not be asked, which is not a verdict. */
async function threadVerdict(record: UnsentMessageRecord): Promise<ThreadVerdict> {
  const found = await ensureThreadByIdInMap(record.threadId);
  // A send that creates its thread names it, so only this send could have
  // made it: the engine took the message.
  if (record.body.new_thread) return found ? 'made-by-this-send' : 'never-made';
  if (!found || threadMap.peek().get(record.threadId)?.meta.state === 'discarded') return 'gone';
  return 'live';
}

/** A retry pass replaces the card rather than stacking a second one. */
const RESTORE_TOAST_KEY = 'unsent-message-restore';

let restoring: Promise<UnsentRestoreOutcome> | null = null;

/** One restore pass over the records this page may adopt. Single-flight. */
export function restoreUnsentMessages(): Promise<UnsentRestoreOutcome> {
  restoring ??= restorePass().finally(() => {
    restoring = null;
  });
  return restoring;
}

async function restorePass(): Promise<UnsentRestoreOutcome> {
  const outcome: UnsentRestoreOutcome = { shown: 0, delivered: 0, movedToDraft: 0, unresolved: 0 };
  const records = await readUnsentMessageStore();
  const liveOwners = await liveOwnerIds();
  // Oldest first, so a thread's cards and any fresh drafts keep the send order.
  records.sort((a, b) => (a.sentAt < b.sentAt ? -1 : a.sentAt > b.sentAt ? 1 : 0));

  for (const record of records) {
    if (record.ownerId === pageOwnerId || ownedByAnotherLiveTab(record.ownerId, liveOwners)) continue;
    // An earlier pass already brought it back.
    if (unsentMessages.peek().has(record.eventId)) continue;
    let verdict: ThreadVerdict;
    try {
      verdict = await threadVerdict(record);
    } catch (err) {
      outcome.unresolved += 1;
      outcome.unresolvedReason ??= errorDetail(err);
      continue;
    }
    const { threadId, eventId, body, settlement } = record;
    if (verdict === 'gone') {
      forgetUnsentMessageRecord(eventId);
      composeInFreshDraft(body.message, body.image_hashes ?? []);
      outcome.movedToDraft += 1;
      continue;
    }
    const answer = record.answersQuestion ? { toolUseId: record.answersQuestion, text: record.body.message } : false;
    if (verdict === 'made-by-this-send' || engineRecordedMessage(threadId, eventId, answer)) {
      forgetUnsentMessageRecord(eventId);
      settleAcceptedSend(threadId, settlement);
      outcome.delivered += 1;
      continue;
    }
    adoptUnsentMessageRecord(record);
    if (verdict === 'never-made') addNeverMadeThread(record);
    if (settlement.kind === 'first-send') resumeUnsentFirstSend(threadId, settlement.engineDraftAtSend);
    showRestoredUnsentMessage(record);
    outcome.shown += 1;
  }

  const message = unsentRestoreMessage(outcome);
  if (message) showToast(message, 'warning', { key: RESTORE_TOAST_KEY });
  return outcome;
}

/** The thread a raw new send would have made, as `sendMessage` drew it. Retry
 *  still carries `new_thread`, so the engine makes it then. */
function addNeverMadeThread(record: UnsentMessageRecord): void {
  const map = new Map(threadMap.peek());
  map.set(record.threadId, makeOptimisticThreadState({
    id: record.threadId,
    title: record.body.message.slice(0, 40),
    channel: record.body.use_coding_agent ? 'claude_code' : 'chat',
    initiator: 'user',
    eventsLoaded: true,
  }));
  threadMap.value = map;
}

let pendingUnresolved = false;

function runRestore(): void {
  restoreUnsentMessages().then((outcome) => {
    pendingUnresolved = outcome.unresolved > 0;
  }).catch((err) => {
    showToast(`Could not bring back unsent messages from before the reload: ${errorDetail(err)}`, 'error');
  });
}

/** Start restoring once the engine has served the thread list, and again on
 *  reconnect for anything a pass could not check. Returns the teardown. */
export function installUnsentMessageRestore(): () => void {
  holdPageOwnerLock();
  let ran = false;
  let wasConnected = isConnected.peek();
  const stops = [
    effect(() => {
      if (ran || !threadListFetched.value) return;
      ran = true;
      runRestore();
    }),
    effect(() => {
      const connected = isConnected.value;
      if (connected && !wasConnected && ran && pendingUnresolved) runRestore();
      wasConnected = connected;
    }),
  ];
  return () => {
    for (const stop of stops) stop();
  };
}

export function _resetUnsentMessageRestoreForTesting(): void {
  restoring = null;
  pendingUnresolved = false;
}
