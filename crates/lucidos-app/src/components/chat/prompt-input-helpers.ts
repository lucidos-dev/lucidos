import { signal } from '@preact/signals';
import { effectiveThreadStatus, focusedThreadId, isMidTurn } from '../../store/store';
import { awaitedQuestionCard, computeExchanges, findQuestionAnswer } from '../../store/thread-events';
import type { ThreadState, ThreadStatus } from '../../store/thread-events';
import { uploadsGate, type PendingUpload } from '../../store/pendingUploads';
import { pendingAnswers } from '../../store/pendingDecisions';

// Pure prompt-input logic + the optimistic-send signal. Extracted from
// PromptInput.tsx (re-exported there); imported directly by *.test.ts.

/** Thread IDs where Send was just clicked but the thread hasn't reached
 *  running/waiting_for_user_answer yet. Drives the optimistic Send→Cancel
 *  morph so the action slot doesn't flash empty during the request gap.
 *  Cleared when the thread becomes cancellable (an effect in PromptInput) or
 *  on send failure (the catch handler in PromptInput's `beginSend`). */
export const submittingThreadIds = signal<Set<string>>(new Set());

/** Set by the Stop shortcut, consumed by the one mounted PromptInput. The
 *  composer owns the cancel (its settle window, its queued-upload branch), so
 *  the shortcut asks it rather than cancelling behind its back. */
export const promptStopRequested = signal(false);

/** Set by the Side question shortcut, consumed by the one mounted PromptInput.
 *  Only the composer knows whether this draft can be asked aside. */
export const promptSideQuestionRequested = signal(false);

/** Why this thread cannot turn on side-question mode, or null when it can.
 *  The hold on the row's end button and the Side question shortcut share it.
 *  Both only ever turn the mode on, whatever the box holds. */
export function sideQuestionBlocker(args: {
  threadStarted: boolean;
  isCodex: boolean;
}): string | null {
  if (!args.threadStarted) return 'Side questions need a started thread.';
  if (args.isCodex) return "Codex threads don't take side questions.";
  return null;
}

/** Whether side-question mode is in force. The stored flag counts only where a
 *  side question can be asked: a started, non-Codex thread. While it is on it
 *  wins over a pending question card, so the box asks rather than answers. */
export function sideQuestionModeActive(args: {
  stored: boolean;
  threadStarted: boolean;
  isCodex: boolean;
}): boolean {
  return args.stored && args.threadStarted && !args.isCodex;
}

export interface UploadSendIntent<TContext = unknown> {
  useCodingAgent: boolean;
  context: TContext | null;
  /** Ask the draft as a side question rather than send it. */
  asSideQuestion?: boolean;
}

/** Thread sends the user clicked while an attached image was still uploading.
 *  The draft stays intact and the actual send is retried once the pending
 *  upload entries settle into confirmed draft hashes. While queued, the same
 *  optimistic submitting signal as a normal send drives the Send→Cancel morph,
 *  and the composer shows the wait (`uploadSendNotice`). */
export const queuedUploadSends = signal<Map<string, UploadSendIntent>>(new Map());

export function queueUploadSend<TContext>(
  threadId: string,
  intent: UploadSendIntent<TContext>,
): void {
  const next = new Map(queuedUploadSends.value);
  next.set(threadId, intent as UploadSendIntent);
  queuedUploadSends.value = next;
  markSubmittingThread(threadId);
}

export function takeQueuedUploadSend(threadId: string): UploadSendIntent | null {
  const intent = queuedUploadSends.value.get(threadId);
  if (!intent) return null;
  const next = new Map(queuedUploadSends.value);
  next.delete(threadId);
  queuedUploadSends.value = next;
  return intent;
}

export function clearQueuedUploadSend(threadId: string): void {
  if (!queuedUploadSends.value.has(threadId)) return;
  const next = new Map(queuedUploadSends.value);
  next.delete(threadId);
  queuedUploadSends.value = next;
  clearSubmittingThread(threadId);
}

/** Threads where a send was refused, or a queued one released, because an
 *  attached image failed to upload. The composer says why until the failed
 *  image is retried or removed. */
export const uploadBlockedSends = signal<Set<string>>(new Set());

export function markUploadBlockedSend(threadId: string): void {
  if (uploadBlockedSends.value.has(threadId)) return;
  uploadBlockedSends.value = new Set(uploadBlockedSends.value).add(threadId);
}

export function clearUploadBlockedSend(threadId: string): void {
  if (!uploadBlockedSends.value.has(threadId)) return;
  const next = new Set(uploadBlockedSends.value);
  next.delete(threadId);
  uploadBlockedSends.value = next;
}

/** Settle each queued send once its uploads do. Every queued send ends in one
 *  of three ways: dispatched, cancelled by the user, or released here.
 *  - In flight, retries and an outage included: it stays queued.
 *  - A terminal failure: released, the draft untouched, and the upload line
 *    says why until nothing on the thread is failed.
 *  - Clear: handed to `dispatch`. */
export function settleQueuedUploadSends(dispatch: (threadId: string, intent: UploadSendIntent) => void): void {
  for (const threadId of uploadBlockedSends.value) {
    if (uploadsGate(threadId) !== 'failed') clearUploadBlockedSend(threadId);
  }
  for (const [threadId] of queuedUploadSends.value) {
    const gate = uploadsGate(threadId);
    if (gate === 'in-flight') continue;
    if (gate === 'failed') {
      clearQueuedUploadSend(threadId);
      markUploadBlockedSend(threadId);
      continue;
    }
    const intent = takeQueuedUploadSend(threadId);
    if (intent) dispatch(threadId, intent);
  }
}

/** What the composer's upload line says about a send:
 *  - `queued`: Send was pressed and waits on `inFlight` images;
 *  - `blocked`: a send was refused or released, `failed` images being why. */
export type UploadSendNotice =
  | { kind: 'queued'; inFlight: number; asSideQuestion: boolean }
  | { kind: 'blocked'; failed: number };

/** The notice for one thread, or null when there is nothing to say. Pure. */
export function uploadSendNotice(
  queued: UploadSendIntent | undefined,
  blocked: boolean,
  uploads: readonly PendingUpload[],
): UploadSendNotice | null {
  const failed = uploads.filter((u) => u.state.kind === 'failed').length;
  if (blocked && failed > 0) return { kind: 'blocked', failed };
  if (!queued) return null;
  return { kind: 'queued', inFlight: uploads.length - failed, asSideQuestion: queued.asSideQuestion === true };
}

export function uploadSendNoticeText(notice: UploadSendNotice): string {
  if (notice.kind === 'blocked') {
    return notice.failed === 1
      ? 'Not sent: an image failed to upload. Retry it or remove it, then send.'
      : `Not sent: ${notice.failed} images failed to upload. Retry or remove them, then send.`;
  }
  const verb = notice.asSideQuestion ? 'Asks' : 'Sends';
  return notice.inFlight > 1
    ? `${verb} when the ${notice.inFlight} images finish uploading`
    : `${verb} when the image finishes uploading`;
}

function markSubmittingThread(threadId: string): void {
  const next = new Set(submittingThreadIds.value);
  next.add(threadId);
  submittingThreadIds.value = next;
}

export function clearSubmittingThread(threadId: string): void {
  if (!submittingThreadIds.value.has(threadId)) return;
  const next = new Set(submittingThreadIds.value);
  next.delete(threadId);
  submittingThreadIds.value = next;
}

// --- Post-submit cancel settle window ------------------------------------
//
// The prompt-row Send/Submit button morphs IN PLACE into a destructive
// Cancel/Stop the instant the user submits: a normal Send flips to the
// running-turn Stop (via the optimistic submitting flag above), and a typed
// answer's Submit flips to a lone Cancel once the draft clears. On a laggy iOS
// PWA the user taps the same spot several times before the UI catches up, so a
// queued or reflexive repeat tap lands on the freshly-morphed Cancel and aborts
// the turn they just started — stamping the next pending question `Canceled` +
// `ResponseCanceled { user_stop }`. (Thread fe390597: a FreeText answer's Submit
// at 15:54:02 → a `Canceled` answer 4.8s later; workspace-wide nearly every
// user_stop cancel sits on a `waiting_for_user_answer` question.)
//
// After a constructive submit we hold the destructive morph DISABLED for this
// long so the burst is absorbed. A genuine stop is one tap away once the window
// passes; a fresh question with no preceding submit is never armed, so the
// escape-hatch Cancel stays immediately usable.
export const CANCEL_SETTLE_MS = 1200;

// Epoch-ms the settle window ends; 0 = not settling. A signal so the prompt
// re-renders the Cancel/Stop enabled⇄disabled as the window arms and expires.
const cancelSettleUntil = signal(0);
let cancelSettleTimer: ReturnType<typeof setTimeout> | null = null;

/** Arm the settle window — call the moment a constructive prompt-row action
 *  (Send / Submit answer) fires, before the button can morph to Cancel/Stop. */
export function armCancelSettle(now: number = Date.now()): void {
  cancelSettleUntil.value = now + CANCEL_SETTLE_MS;
  if (cancelSettleTimer) clearTimeout(cancelSettleTimer);
  // Flip the signal back off so the component re-renders the button enabled
  // again once the window passes (no other render trigger is guaranteed).
  cancelSettleTimer = setTimeout(() => {
    cancelSettleUntil.value = 0;
    cancelSettleTimer = null;
  }, CANCEL_SETTLE_MS);
}

/** True while the post-submit settle window is active: the destructive
 *  Cancel/Stop morph must render disabled and ignore taps. Reactive — reading
 *  it in render subscribes the component to the arm/expire transitions. */
export function isCancelSettling(now: number = Date.now()): boolean {
  const until = cancelSettleUntil.value;
  return until !== 0 && now < until;
}

// What Escape does while the prompt textarea has focus.
//   leave-side-question: the composer is in side-question mode, so Escape turns
//           it off and keeps the text. It goes first, as the least destructive.
//   cancel: there is something to cancel, so Escape is the keyboard twin of the
//           row's red button: abort the running turn, or stamp the pending
//           question `Canceled`.
//   blur:   nothing to cancel (composing, or an idle thread). Escape drops the
//           caret out of the composer, which is what it always did.
//   ignore: the post-submit settle window. The destructive control is held
//           disabled there so a laggy repeat tap can't abort the turn the user
//           just started (see armCancelSettle), and Escape is the same hazard
//           one key away: submit with Enter, reflex Escape, turn gone. Nothing
//           happens for that window, deliberately, rather than falling through
//           to `blur` and teaching the user that Escape sometimes only blurs.
export type PromptEscapeAction = 'leave-side-question' | 'cancel' | 'blur' | 'ignore';

export function computePromptEscapeAction(
  hasCancelTarget: boolean,
  settling: boolean,
  sideQuestionMode: boolean,
): PromptEscapeAction {
  if (sideQuestionMode) return 'leave-side-question';
  if (!hasCancelTarget) return 'blur';
  return settling ? 'ignore' : 'cancel';
}

/** Is there anything to send? ONE reading, and both the Send face's lit-ness
 *  and `submit()`'s dispatch take it.
 *
 *  Two readings is what a dead Send button looks like. The face was lit from
 *  `text.length > 0` and the send refused on `text.trim()`. A draft of nothing
 *  but spaces therefore drew an enabled button whose press returned in silence.
 *  A FAILED image upload did the same from the other side: it counted as
 *  content but not as in-flight, so nothing was sendable and the button said
 *  otherwise.
 *
 *  The text arrives TRIMMED here, because that is what a send carries. An
 *  upload arrives as a boolean rather than a count, because only one of the two
 *  pending states is a reason to light Send: an `uploading` entry becomes a
 *  send the moment its hash lands, and a `failed` one never will. */
export function composeHasContent(
  text: string,
  attachedImagesCount: number,
  uploadInFlight: boolean,
): boolean {
  return text.trim().length > 0 || attachedImagesCount > 0 || uploadInFlight;
}

/** What a submit sends, out of the two places the composer keeps its text.
 *
 *  **The draft decides.** The Send face is rendered from it, through
 *  `composeHasContent` above, and `sendCompose` sends it. Gating on the textarea
 *  instead lit the button from one value and refused it from another. The two
 *  need only drift apart for the press to die in silence, which is what four
 *  reports of a dead composer button look like. The plan behind this is
 *  docs/plans/2026-08-27-the-composer-sends-the-draft-it-is-showing.md.
 *
 *  The textarea is not ignored. Text the store lacks is sent, and handed back to
 *  the store as well. So nothing typed is lost, and `sendCompose` cannot go on
 *  to send an empty copy. `domText` is null when there is no textarea node,
 *  which is an absent source rather than a disagreement.
 *
 *  Both are compared trimmed, matching what the submit paths already send, so
 *  trailing whitespace never reads as a disagreement. */
export interface ComposerText {
  /** The trimmed text to send, and what the caller's own empty check reads. */
  text: string;
  /** The RAW textarea value to write into the store before dispatching, or null
   *  when the store already holds the text. Raw, so the recovery alters nothing
   *  the user typed. */
  storeWrite: string | null;
  /** The two sources held different text. The impossible state itself, so the
   *  caller reports it. */
  disagreed: boolean;
}

export function resolveComposerText(draftText: string, domText: string | null): ComposerText {
  const draft = draftText.trim();
  if (domText === null) return { text: draft, storeWrite: null, disagreed: false };
  const dom = domText.trim();
  if (draft === dom) return { text: draft, storeWrite: null, disagreed: false };
  if (draft.length > 0) return { text: draft, storeWrite: null, disagreed: true };
  return { text: dom, storeWrite: domText, disagreed: true };
}

/** What the user is told when the two disagreed, and null when they agreed. It
 *  names which copy was sent, because the box may be showing the other one.
 *
 *  It takes the whole resolution rather than a side, so neither caller repeats
 *  the reading of `storeWrite` that decides which copy that was. */
export function composerTextDisagreementToast(resolved: ComposerText): string | null {
  if (!resolved.disagreed) return null;
  return resolved.storeWrite === null
    ? 'The text on screen and the saved draft differed. Sent the saved draft.'
    : 'The saved draft was empty. Sent the text on screen.';
}

/** Whether the optimistic `submittingThreadIds` flag should be released.
 *
 *  **Stop is offered only while the focused thread has a turn in flight**, and
 *  this predicate is the half of that rule the real status cannot express. The
 *  flag exists to bridge the click → SSE gap right after Send, when the thread
 *  has not reported `running` yet; every frame it survives past that gap is a
 *  frame showing a red Stop with nothing behind it. Since the thread-level Stop
 *  stopped ending subscriptions, such a Stop does *nothing at all* when pressed.
 *
 *  Two ways to release, and the second is the one that was missing:
 *
 *  - **The real status took over** (`isMidTurn`). The bridge did its job and
 *    `getWaitingState` drives the same button from here on.
 *  - **Nothing is in flight.** A turn that settles without the client ever
 *    observing `running` never hit the first case, so the flag stuck for the
 *    life of the page. The reproducible shape is a send that fails outright: the
 *    thread goes straight to `failed`, which is not mid-turn, and the Stop
 *    stayed on screen on an idle thread until reload.
 *
 *  `status` is the EFFECTIVE status, which already folds a confirmed pending
 *  user message into `running`, so an in-flight send is covered by the first
 *  case and the second cannot cut the bridge short. A *queued upload send* is
 *  the one pending thing the status does not know about: no turn is running,
 *  but a real send is waiting on an image hash and Stop drops it, so the flag
 *  is held until the upload settles. */
export function shouldClearSubmitting(status: ThreadStatus, uploadSendQueued: boolean): boolean {
  if (isMidTurn(status)) return true;
  return !uploadSendQueued;
}

// The button is always rendered EXCEPT in 'hidden' mode so Send↔Cancel keeps
// its color morph without a DOM swap; the leave path snap-unmounts like the
// sibling section buttons — no fade-out, no position:absolute jump.
//   send        — visible, blue, click=submit          (up-arrow icon)
//   cancel      — visible, red,  click=cancel exchange  (stop-square icon)
//   canceling   — visible, red,  disabled               (stop-square icon, dimmed)
//   placeholder — invisible (visibility:hidden, takes space) to keep row height
//   hidden      — not rendered; banner or section buttons own the slot
type MorphMode = 'send' | 'cancel' | 'canceling' | 'placeholder' | 'hidden';

export function computeMorphMode(args: {
  hasContent: boolean;
  cancelTargetId: string | null;
  isCanceling: boolean;
  hasBannerOrSectionButtons: boolean;
}): MorphMode {
  if (args.hasContent) return 'send';
  if (args.cancelTargetId !== null) return args.isCanceling ? 'canceling' : 'cancel';
  if (args.hasBannerOrSectionButtons) return 'hidden';
  return 'placeholder';
}

// What the prompt-row action shows while the thread is `waiting_for_user_answer`
// (a pending user question OR permission). The morph button (computeMorphMode)
// is NOT used in this state — this control replaces it so "Stop" is never the
// prominent default while the agent is waiting on the user.
//   canceling — disabled "Canceling…" (a Cancel is in flight)
//   multi     — split button: Submit (N) primary + caret → red Cancel menu.
//               Multi-select always shows Submit (disabled at zero selections),
//               so the caret is the only place a Cancel fits — hence the split.
//   submit    — lone green Submit: a freetext/custom answer has been typed.
//   cancel    — lone red Cancel: nothing to submit (single-select / permission /
//               empty freetext); the forward action lives in the card above.
export type AnswerActionMode = 'canceling' | 'multi' | 'submit' | 'cancel';

export function computeAnswerActionMode(args: {
  pendingMultiQ: boolean;
  hasContent: boolean;
  isCanceling: boolean;
}): AnswerActionMode {
  if (args.isCanceling) return 'canceling';
  if (args.pendingMultiQ) return 'multi';
  if (args.hasContent) return 'submit';
  return 'cancel';
}

// Prompt placeholders. Typing here while a question card is pending IS the
// user's answer to it (the engine reroutes the text as `AnswerKind::FreeText`),
// so the placeholder says so. Keyed on a pending question card rather than the
// `waiting_for_user_answer` status, which also covers coding-agent permission
// cards: those absorb no typed text, so inviting an answer there would be a lie.
//
// The answering placeholder names the typing escape, and says "custom answer"
// rather than "your answer" on purpose: it has to read as a peer of the card's
// options, not as composer chrome the eye skips. It is kept to one short line
// for the same reason. A longer one wraps to two lines at 125% UI scale in
// monospace on a phone, and a long line of grey text is the most skipped thing
// on screen. The other escape is named by the lone Cancel button's tooltip in
// PromptInput.tsx (`ANSWER_CANCEL_TOOLTIP`). Between the two, nothing on the
// card needs an "Other, I'll type it" option row, which would just hand its own
// label back as the answer.
//
// A thread parked on an *event wait* deliberately gets NO placeholder of its
// own. It is asleep on the SYSTEM, not on the user, the composer stays fully
// enabled, and a message runs an ordinary turn while the subscription keeps
// waiting: that is exactly the ordinary follow-up promise, so the ordinary
// follow-up line says it. The event-wait indicator and the wait card already
// carry the state; a second copy in the composer only added a line of grey
// text under every parked thread.
export const PLACEHOLDER_NEW_THREAD = 'What can I help with?';
export const PLACEHOLDER_FOLLOW_UP = 'Post a follow up…';
export const PLACEHOLDER_ANSWERING = 'Type custom answer here…';
export const PLACEHOLDER_SIDE_QUESTION = 'Ask a side question…';

/** Side-question mode wins over a pending question card: in the mode, Send
 *  asks the side question, and the × goes back to answering the card. */
export function promptPlaceholder(
  hasFocusedThread: boolean,
  answeringQuestionCard: boolean,
  sideQuestionMode: boolean,
): string {
  if (sideQuestionMode) return PLACEHOLDER_SIDE_QUESTION;
  if (answeringQuestionCard) return PLACEHOLDER_ANSWERING;
  return hasFocusedThread ? PLACEHOLDER_FOLLOW_UP : PLACEHOLDER_NEW_THREAD;
}

// Stamp cancelTargetId BEFORE invoking send. sendCompose's sync prefix
// clears the draft and flips state→'active' (section buttons appear); if
// cancelTargetId is still null at that render, morphMode resolves to
// 'hidden', the button unmounts, and Send→Cancel blinks instead of morphing.
// Raw new sends (threadId null) have no prior button to preserve and pick up
// the new id from focusedThreadId after send's sync prefix runs setFocusedThread.
export function dispatchSend(
  threadId: string | null,
  send: () => Promise<unknown>,
): { promise: Promise<void>; submittedId: string | null } {
  if (threadId) {
    markSubmittingThread(threadId);
  }
  const promise = send().then(() => undefined);
  const submittedId = threadId ?? focusedThreadId.value;
  if (!threadId && submittedId) {
    markSubmittingThread(submittedId);
  }
  return { promise, submittedId };
}

// Toggled options + the textarea's custom answer each count as one selection.
// Whitespace-only text is dropped to mirror submitMultiAnswer's text.trim().
export function computeSubmitMultiCount(toggledCount: number, customAnswerText: string): number {
  return toggledCount + (customAnswerText.trim().length > 0 ? 1 : 0);
}

/** What a FAILED multi-select answer hands back to the composer.
 *
 *  `submitMultiAnswer` clears the toggles and the box before the POST, because
 *  that clearing is the send gesture. When the POST fails the card comes back
 *  live and blank, so the user re-picks every option. The reported failure cost
 *  three picks twice over before the third attempt landed.
 *
 *  Only where the user has put nothing new there since. A fresh pick made
 *  during the failure window is the newer answer. Restoring text over a fresh
 *  keystroke would also leave the draft and the box disagreeing, and
 *  `resolveComposerText` settles that in favour of the stale copy. `null` for
 *  either field means leave that one alone. */
export function recoverableAnswerDraft(args: {
  sentIds: string[];
  sentText: string;
  currentIds: string[];
  currentDraft: string;
  /** The textarea's live value, or null when no node is mounted. */
  domText: string | null;
}): { ids: string[] | null; text: string | null } {
  const idsFree = args.currentIds.length === 0;
  const textFree = args.currentDraft.length === 0 && (args.domText ?? '').length === 0;
  return {
    ids: args.sentIds.length > 0 && idsFree ? args.sentIds : null,
    text: args.sentText.length > 0 && textFree ? args.sentText : null,
  };
}

/** Latest unanswered `UserQuestionAsked` on the thread (single OR multi) —
 *  each pending question lives in its own divider exchange (the
 *  `UserQuestionAsked` is the exchange's `userEvent`). Returns `null` when the
 *  latest question is already answered: the engine serializes questions (one
 *  pending at a time via `walk_question_batch`), so an answered latest means
 *  nothing is pending. Callers must gate by status; this walks every exchange
 *  and is too expensive to run on every keystroke otherwise.
 *
 *  One walk, two facts, deliberately: PromptInput needs both per render, and
 *  derives each from the returned object rather than walking again. The
 *  question's mere presence picks the answer-oriented placeholder; its
 *  `multiSelect` picks the prompt-row Submit control (single-select answers
 *  through the card itself, so the multi-submit path ignores it). */
export function findLatestPendingQuestion(
  thread: ThreadState | undefined,
): { toolUseId: string; multiSelect: boolean } | null {
  if (!thread) return null;
  const exchanges = computeExchanges(thread);
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ex = exchanges[i];
    const ue = ex.userEvent;
    if (ue.type !== 'UserQuestionAsked') continue;
    if (findQuestionAnswer(ex, ue.tool_use_id)) return null;
    return { toolUseId: ue.tool_use_id, multiSelect: !!ue.multi_select };
  }
  return null;
}

/** The question card a message sent now would answer, by tool-use id. The
 *  engine routes typed text to an open question as its `FreeText` answer.
 *
 *  Only the NEWEST card that awaits an answer counts. A permission card waits
 *  under the same status, and walking past it would reach a question an abort
 *  stranded. A send behind an answer already in flight answers nothing either,
 *  typed or tapped: the engine takes the later text as an ordinary message. */
export function questionTheSendAnswers(thread: ThreadState): string | undefined {
  if (effectiveThreadStatus(thread) !== 'waiting_for_user_answer') return undefined;
  const card = awaitedQuestionCard(computeExchanges(thread));
  return card && !pendingAnswers.map.peek().has(card) ? card : undefined;
}

/** Whether the optimistic `cancelingThreadIds` flag should be released.
 *
 *  The flag bridges the click→SSE gap after Cancel so the morph button reads
 *  "Cancel..." (disabled) and a double-tap can't re-fire. It must drop once the
 *  cancel has landed, or the button sticks disabled until reload. Which release
 *  condition applies depends on WHAT the cancel targeted at click time:
 *
 *   - **Question-card cancel** (`canceledQuestionId` set): released when the
 *     turn fully ended OR the targeted question is no longer the thread's latest
 *     pending one (`latestPendingQuestionId` differs) — it resolved as Canceled
 *     and the agent idled or re-asked. This is the re-ask case a not-mid-turn
 *     check misses: status stays mid-turn the whole time
 *     (waiting_for_user_answer → running → waiting_for_user_answer).
 *
 *   - **Card cancel with no question id** (`canceledWhileAwaitingAnswer`, i.e. a
 *     coding-agent permission card — those are not `UserQuestionAsked`, so they
 *     record no `canceledQuestionId`): bridge the gap until the turn leaves
 *     every mid-turn state.
 *
 *   - **Generic running-turn cancel** (neither of the above): release the moment
 *     the turn is no longer `running` — whether it terminated OR paused on a NEW
 *     `waiting_for_user_answer` card the cancel never targeted. The latter is the
 *     superseded-cancel case (a follow-up redirect swallowed the cancel, or the
 *     agent answered a running-turn Stop by asking a question): treating
 *     `waiting_for_user_answer` as still-mid-turn here wedges "Canceling"
 *     forever (Codex incident, 2026-07-03). */
export function shouldClearCanceling(
  status: ThreadStatus,
  canceledQuestionId: string | undefined,
  latestPendingQuestionId: string | undefined,
  canceledWhileAwaitingAnswer = false,
): boolean {
  if (canceledQuestionId !== undefined) {
    if (!isMidTurn(status)) return true;
    return latestPendingQuestionId !== canceledQuestionId;
  }
  if (canceledWhileAwaitingAnswer) return !isMidTurn(status);
  return status !== 'running';
}
