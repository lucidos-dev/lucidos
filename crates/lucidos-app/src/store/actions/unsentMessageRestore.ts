
import { isHistoricalRestoreSource } from '../../utils/restoreSourceGeneration';
import { effect } from '@preact/signals';
import { waitForRestoreAdmission, restoreSourcePreserved } from '../../utils/restoreAdmission';

import { awaitedQuestionCard, computeExchanges, makeOptimisticThreadState } from '../thread-events';
import { holdPageOwnerLock, liveOwnerIds, ownedByAnotherLiveTab, pageOwnerId } from '../pageOwner';
import { isConnected, showToast, threadListFetched, threadMap } from '../store';
import {
  adoptUnsentMessageRecord,
  forgetUnsentMessageRecord,
  readUnsentMessageStore,
  type UnsentMessageRecord,
} from '../unsentMessageRecords';
import { unsentPicks } from '../pendingDecisions';
import { unsentMessages } from '../unsentMessages';
import { errorDetail } from '../../utils/errorDetail';
import { rerunWhenPageShows } from '../deviceDatabase';
import { engineRecordedMessage, showRestoredUnsentMessage } from './chat';
import { composeInFreshDraft, resumeUnsentFirstSend } from './compose';
import { settleAcceptedSend } from './sendSettlement';
import { ensureThreadByIdInMap } from './thread-loading';
import { recoverQuestionPick } from './questionPickRecovery';

export interface UnsentRestoreOutcome {
  shown: number;
  delivered: number;
  movedToDraft: number;
  unresolved: number;
  unresolvedReason?: string;
}

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

async function threadVerdict(record: UnsentMessageRecord): Promise<ThreadVerdict> {
  const found = await ensureThreadByIdInMap(record.threadId);
  if (record.body.new_thread) return found ? 'made-by-this-send' : 'never-made';
  if (!found || threadMap.peek().get(record.threadId)?.meta.state === 'discarded') return 'gone';
  return 'live';
}

const RESTORE_TOAST_KEY = 'unsent-message-restore';

let restoring: Promise<UnsentRestoreOutcome> | null = null;

export function restoreUnsentMessages(): Promise<UnsentRestoreOutcome> {
  restoring ??= restorePass().finally(() => {
    restoring = null;
  });
  return restoring;
}

async function restorePass(): Promise<UnsentRestoreOutcome> {
  await waitForRestoreAdmission();
  const outcome: UnsentRestoreOutcome = { shown: 0, delivered: 0, movedToDraft: 0, unresolved: 0 };
  const stored = await readUnsentMessageStore();
  await waitForRestoreAdmission();
  const records = stored.filter((r) => !isHistoricalRestoreSource(r) && !restoreSourcePreserved('send', r.eventId));
  const liveOwners = await liveOwnerIds();
  await waitForRestoreAdmission();
  records.sort((a, b) => (a.sentAt < b.sentAt ? -1 : a.sentAt > b.sentAt ? 1 : 0));

  for (const record of records) {
    if (isHistoricalRestoreSource(record)) continue;
    if (record.ownerId === pageOwnerId || ownedByAnotherLiveTab(record.ownerId, liveOwners)) continue;
    if (unsentMessages.peek().has(record.eventId)) continue;
    let verdict: ThreadVerdict;
    try {
      verdict = await threadVerdict(record);
      await waitForRestoreAdmission();
      if (isHistoricalRestoreSource(record)) continue;
    } catch (err) {
      outcome.unresolved += 1;
      outcome.unresolvedReason ??= errorDetail(err);
      continue;
    }
    const { threadId, eventId, body, settlement } = record;
    if (verdict === 'gone') {
      composeInFreshDraft(body.message, body.image_hashes ?? []);
      forgetUnsentMessageRecord(eventId);
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
    if (record.questionPick) {
      const { toolUseId, answer, placement } = record.questionPick;
      const pick = { threadId, answer, placement, refusal: record.refusal };
      unsentPicks.set(toolUseId, pick);
      const thread = threadMap.peek().get(threadId);
      if (!thread || thread.meta.status !== 'waiting_for_user_answer' || awaitedQuestionCard(computeExchanges(thread)) !== toolUseId) {
        recoverQuestionPick(toolUseId, pick, body.message);
        outcome.shown += 1;
        continue;
      }
    } else {
      showRestoredUnsentMessage(record);
    }
    outcome.shown += 1;
  }

  const message = unsentRestoreMessage(outcome);
  if (message) showToast(message, 'warning', { key: RESTORE_TOAST_KEY });
  return outcome;
}

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
    if (rerunWhenPageShows(err, runRestore)) return;
    showToast(`Could not bring back unsent messages from before the reload: ${errorDetail(err)}`, 'error');
  });
}

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
