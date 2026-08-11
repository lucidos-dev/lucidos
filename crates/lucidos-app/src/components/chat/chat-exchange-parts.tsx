import { blobPreviewUrl, continueThread, postCommandCheckpointUndo } from '../../api/client';
import type { Change } from '../../api/client';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { ensureChangeLoaded, revertChange } from '../../store/actions/chat-changes';
import { ensureEventTargetResolved, eventHasTarget, showEventWhereItLives } from '../../store/actions/event-navigation';
import { viewChangeDiff } from '../../store/actions/repositories';
import { checkpointDiffModal, contextViewer, findChangeById, lazyChanges, openImagePopupFromGroup, showToast, stepDetailModal } from '../../store/store';
import { LUCIDOS_AGENT_LABEL, eventWaitStoppedSummary, isThinking, resumeEngineNote, stepStatus } from '../../store/thread-events';
import { LucidosGlyph } from '../shared/LucidosMark';
import { BlobImage } from '../shared/BlobImage';
import type { EventWaitCancelCause, Exchange } from '../../store/thread-events';
import type { Loadable, ResponseEvent } from '../../store/types';
import type { CodingAgent } from '../../api/types';
import { errorDetail } from '../../utils/errorDetail';
import { formatFileCount } from '../../utils/formatFileCount';
import { formatShortDate, formatShortTime, isSameDayInUserTz } from '../../utils/formatTime';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { eventNameChip, eventRowBody } from './EventRow';
import type { EventRowFact, EventRowMark, EventRowTone } from './EventRow';
import { followContinuedThread } from './scrollState';
import { contextPercent, formatTokens } from '../../utils/formatTokens';
import { ClaudeIcon, CodexIcon, CollapseTurnIcon, FullResponseIcon, StepLogIcon } from '../shared/icons';
import { highlightEllipsis } from './highlightEllipsis';
import { getSessionBlobUrlForHash } from './pastedImages';
import { useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';
import type { InitiatorDescriptor } from './ChatExchange';

// Presentational sub-components for ChatExchange (panels, bodies, response
// rendering). Extracted from ChatExchange.tsx; imported back there. The only
// dependency on the parent is the InitiatorDescriptor type (erased at runtime).

const CHANGE_ACCENT = {
  ChangeApplied: 'change-applied',
  ChangeDiscarded: 'change-discarded',
  ChangeReverted: 'change-reverted',
} as const;

export function changeAccent(type: keyof typeof CHANGE_ACCENT): string {
  return CHANGE_ACCENT[type];
}

export function MarkdownBlock({ html }: { html: string }) {
  return <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />;
}

export function FileList({ files }: { files: string[] }) {
  return (
    <ul class="initiator-files">
      {files.map(f => <li key={f}><code>{f}</code></li>)}
    </ul>
  );
}

export function UserMessageBody({ html, imageHashes }: { html: string; imageHashes: string[] }) {
  return (
    <>
      {html && <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />}
      {imageHashes.length > 0 && (
        <div class="user-images">
          {imageHashes.map((hash, i) => {
            const src = getSessionBlobUrlForHash(hash) ?? blobPreviewUrl(hash);
            return (
              <BlobImage
                key={hash + ':' + i}
                src={src}
                class="user-image-thumb"
                alt=""
                onClick={(e) => openImagePopupFromGroup(e.currentTarget.src, e.currentTarget)}
              />
            );
          })}
        </div>
      )}
    </>
  );
}

/** Body for change lifecycle initiator panels — surfaces the change description,
 *  file count, and (when failed) the error message. The resolved timestamp is
 *  the panel header timestamp (the change-lifecycle event time IS resolved_at).
 *
 *  `seedDescription`/`seedFileCount` come from the in-thread `ChangeProposed`
 *  event (already loaded with the thread). They render the body at its FINAL
 *  height on first open — before the per-id `Change` lazy-fetch lands — so a
 *  thread that ends with "Change applied" doesn't jump when the fetched row
 *  pops the description + file count in (the open-path scroll-to-bottom would
 *  re-pin to the grown bottom, shifting everything up). The authoritative live
 *  row wins once loaded; it's normally identical to the seed. */
export function ChangeBody({ changeId, error, seedDescription, seedFileCount }: { changeId?: string; error?: string; seedDescription?: string; seedFileCount?: number }) {
  const change: Change | undefined = changeId ? findChangeById(changeId) : undefined;
  const lazy: Loadable<Change> = (changeId ? lazyChanges.value.get(changeId) : undefined) ?? { status: 'not-loaded' };
  const showLoading = useDelayedLoading(lazy);

  useEffect(() => {
    if (changeId) void ensureChangeLoaded(changeId);
  }, [changeId]);

  const desc = change ? change.description.split('\n')[0]
    : seedDescription ? seedDescription.split('\n')[0]
    : undefined;
  const fileCount = change?.file_count ?? seedFileCount;

  // Lifecycle error and any body content (live row OR in-thread seed) both win
  // over the lazy-fetch state — a 404 for a row that arrives via SSE moments
  // later shouldn't strand a stale "Failed to load" row, and a seeded body is
  // already complete, so the fetch failure (which only gates the footer's
  // Diff/Revert) must not surface a redundant error line beneath it.
  const lazyFailedError = !error && !desc && fileCount == null && lazy.status === 'failed'
    ? `Failed to load change details: ${lazy.error}`
    : undefined;
  // No "Loading..." line once the seed has already painted the body — the
  // fetch is still running (the footer's Diff/Revert need the live row), but
  // the body is complete, so a Loading line would just be a flicker + a shift.
  const lazyLoading = !desc && fileCount == null && lazy.status === 'loading' && showLoading;

  if (!desc && fileCount == null && !error && !lazyFailedError && !lazyLoading) return null;
  return (
    <div class="change-body">
      {desc && <div class="change-body-desc">{desc}</div>}
      {fileCount != null && (
        <div class="change-body-meta">{formatFileCount(fileCount)}</div>
      )}
      {error && <div class="change-body-error">{error}</div>}
      {lazyFailedError && <div class="change-body-error">{lazyFailedError}</div>}
      {lazyLoading && <div class="change-body-meta">Loading...</div>}
    </div>
  );
}

/** "Continue" button rendered on the abort exchange the user may resume from
 *  (`continuableAbortIndex`). Disables itself between click and response so a
 *  double-click can't double-emit. Surfaces network failures via toast and
 *  re-enables. */
export function ContinueButton({ threadId }: { threadId: string }) {
  const inFlight = useSignal(false);
  const onClick = async (e: MouseEvent) => {
    e.stopPropagation();
    if (inFlight.value) return;
    inFlight.value = true;
    // A SUBMIT: the agent is expected to respond to it, so it gets the same one
    // reaction a send does. Its turn does not exist yet (the continuation renders
    // as a fresh `ContinuationStarted` exchange), so the landing waits for it.
    // Before the awaited POST, because this is the button's own tap and must not
    // wait on the round trip. See `followSubmit`.
    followContinuedThread();
    try {
      await continueThread(threadId);
    } catch (err) {
      showToast(`Failed to continue: ${errorDetail(err)}`, 'error');
      inFlight.value = false;
      return;
    }
    // ContinuationStarted will arrive via SSE and remove the button by hiding
    // this exchange's `isContinuableAbort`. Re-enable as a safety net in case
    // the SSE event is delayed.
    setTimeout(() => { inFlight.value = false; }, 5000);
  };
  return (
    <button class="action-btn" onClick={onClick} disabled={inFlight.value}>
      {inFlight.value ? 'Continuing...' : 'Continue'}
    </button>
  );
}

/** Render the engine note for a ContinuationStarted exchange — a one-line
 *  subline followed by a `<details>` expansion showing the full injected text.
 *  Returns null when no engine note is present (e.g. CC resume path). */
export function ResumeNoteBody({ exchange }: { exchange: Exchange }) {
  const note = resumeEngineNote(exchange);
  if (!note) return null;
  const subline = note.toolCount > 0
    ? `Reminded the model about ${note.toolCount} prior tool call${note.toolCount === 1 ? '' : 's'}`
    : 'Reminded the model that no actions had completed';
  return (
    <details class="resume-note">
      <summary>{subline}</summary>
      <pre class="resume-note-body">{note.text}</pre>
    </details>
  );
}

/** The body of a detached event-wake exchange: the event that woke the thread,
 *  named, with its payload folded away (ADR 0047).
 *
 *  The alternative it replaces is why this exists at all. The wake's
 *  `UserPromptInjected.text` has to spell the payload out as pretty-printed
 *  JSON, because that text IS the prompt the model reads, and rendering it
 *  through `MarkdownBlock` put a screen and a half of raw JSON in the
 *  transcript (with markdown helpfully auto-linking the email-shaped part of a
 *  path inside it). The engine links the anchor back to its
 *  `EventWaitDelivered` precisely so the client can show the same facts as
 *  structure instead.
 *
 *  Closed by default: the event's NAME is the answer to "why did this thread
 *  start talking again", and the payload is for the rare follow-up question.
 *
 *  It names no arming reason, deliberately. The reason lives on the
 *  `EventWaitStarted`, which is routinely outside the loaded window by the time
 *  the wake lands, and a row states no fact its own event carries.
 *
 *  **This is where the jump to the matched event lives** (moved here on
 *  2026-08-10 from the arming card, which is about the moment the wait was SET
 *  UP and had no business linking to something that happened hours later). This
 *  card IS the arrival, so a link out of it goes to the thing that arrived.
 *
 *  **The event's NAME is that jump**, as of 2026-08-10. It used to be a separate
 *  "Go to event" link on the facts line, which put two things to read on the
 *  card for one destination while the chip right above it already named the
 *  destination.
 *
 *  The thin hook-holding wrapper; the markup is `eventDeliveryBody`. */
export function EventDeliveryBody({
  eventType,
  eventId,
  payloadJson,
}: {
  eventType: string;
  eventId?: string;
  payloadJson?: string;
}) {
  // Resolving the matched event's thread is a round-trip in every case except a
  // match in this same thread, so the chip dims and goes inert while the jump
  // is in flight rather than accepting a second tap. See `showEventWhereItLives`.
  const opening = useSignal(false);
  const linkable = useEventTarget(eventId);
  return eventDeliveryBody({
    eventType,
    payloadJson,
    opening: opening.value,
    onOpenMatched: linkable && eventId
      ? async () => {
          if (opening.value) return;
          opening.value = true;
          try {
            await showEventWhereItLives(eventId);
          } finally {
            opening.value = false;
          }
        }
      : undefined,
  });
}

/** Settle whether a jump to `eventId` has anywhere to go, and report it.
 *
 *  False until the answer is known, so a row starts plain and GAINS its link.
 *  The other direction, a link that appears and then disappears, reads as a bug
 *  and can be tapped in the window before it goes.
 *
 *  Shared by the two rows that offer a jump. Resolution happens in the effect
 *  rather than the render body precisely so the row subscribes to the one small
 *  verdict signal and not to `threadMap`. */
function useEventTarget(eventId: string | undefined): boolean {
  useEffect(() => { ensureEventTargetResolved(eventId); }, [eventId]);
  return eventHasTarget(eventId);
}

/** The row's markup, hookless for the same reason `eventWaitRowBody` is: there
 *  is no jsdom in the test infra, so a component carrying a hook cannot be
 *  invoked as a plain function and the tests drive this instead.
 *
 *  `onOpenMatched` absent means there is nowhere to go, and then the event name
 *  is inert text: no link, no affordance, no dead tap. Both of the ways that
 *  happens are ordinary rather than exceptional, which is why the plain form is
 *  the default and not a fallback. The engine may have recorded no `event_id`
 *  for the delivery at all, and a recorded one may point at an event that no
 *  transcript draws (a workspace domain event belongs to no conversation; a
 *  `BackgroundBashCompleted` merely landed inside whichever turn was open). */
export function eventDeliveryBody({
  eventType,
  payloadJson,
  opening,
  onOpenMatched,
}: {
  eventType: string;
  payloadJson?: string;
  opening: boolean;
  onOpenMatched?: () => void;
}) {
  return eventRowBody({
    kind: 'wake',
    mark: 'arrived',
    role: 'event-delivery',
    // The matched event usually lives somewhere ELSE: the headline cases are a
    // `CodingAgentIdled` or a `ChangeProposed` from the coding-agent thread this
    // one was watching. So the jump resolves the owning thread first and
    // navigates there, rather than searching the open thread's DOM for an event
    // that is by construction not in it.
    subject: (
      <>
        {'Woke on '}
        {eventNameChip({
          kind: 'chip',
          name: eventType,
          onClick: onOpenMatched,
          pending: opening,
          role: 'event-delivery-jump',
        })}
      </>
    ),
    stateLabel: 'delivered',
    tone: 'arrived',
    fold: payloadJson ? { label: 'Payload', pre: true, body: payloadJson } : undefined,
  });
}

type TriggerStartedEvent = Extract<Exchange['userEvent'], { type: 'TriggerStarted' }>;

/** A trigger firing, as an **event row**: the same marker an event wait, an
 *  event wake and a child callback use. Something outside the thread happened
 *  and started a turn, which is the one thing all four report.
 *
 *  The prompt goes in the fold. It used to be the whole body, rendered as
 *  markdown, which put a trigger's full instructions above the response it
 *  produced every time one fired.
 *
 *  **It names no schedule.** `TriggerStarted` carries `invocation: { kind:
 *  'Schedule' }` and no cron expression, so a scheduled run says `scheduled` and
 *  stops rather than inventing one. An event-driven run does carry its
 *  `event_type`, and gets the chip.
 *
 *  **That chip is the jump, when there is one.** It was a separate "Go to event"
 *  link beside the chip, and on this row it was usually a guaranteed toast: a
 *  trigger fires on a workspace domain event, which belongs to no conversation
 *  and therefore has no transcript to open. The engine does also fire triggers
 *  on thread-scoped events (`TriggerInvocation::Event` carries an
 *  `origin_thread_id`), and those genuinely do open, so the rule is the same one
 *  the wake card uses rather than a blanket "never link here": the chip links
 *  only when the event turns out to live somewhere.
 *
 *  The thin hook-holding wrapper; the markup is `triggerFiredBody`. */
export function TriggerFiredBody({ event }: { event: TriggerStartedEvent }) {
  const matched = event.invocation?.kind === 'Event' ? event.invocation.event_id : undefined;
  const opening = useSignal(false);
  const linkable = useEventTarget(matched);
  return triggerFiredBody({
    event,
    opening: opening.value,
    onOpenMatched: linkable && matched
      ? async () => {
          if (opening.value) return;
          opening.value = true;
          try {
            await showEventWhereItLives(matched);
          } finally {
            opening.value = false;
          }
        }
      : undefined,
  });
}

/** The row's markup, hookless for the same reason `eventWaitRowBody` is: there
 *  is no jsdom in the test infra, so a component carrying a hook cannot be
 *  invoked as a plain function and the tests drive this instead.
 *
 *  `onOpenMatched` absent means the matched event has nowhere to open, and the
 *  chip is then inert text. See the wrapper above for why that is the common
 *  case on this row rather than the exceptional one. */
export function triggerFiredBody({
  event,
  opening,
  onOpenMatched,
}: {
  event: TriggerStartedEvent;
  opening: boolean;
  onOpenMatched?: () => void;
}) {
  const invocation = event.invocation;
  const name = event.trigger_name?.trim();
  return eventRowBody({
    kind: 'trigger',
    mark: 'arrived',
    role: 'trigger-fired',
    // A trigger with no recorded name says only that one fired. It never falls
    // back to `trigger_id`: that is a uuid, and no screen in Lucidos is
    // labelled with one.
    subject: name ? `Trigger fired: ${name}` : 'Trigger fired',
    stateLabel: 'fired',
    tone: 'arrived',
    facts: [
      invocation?.kind === 'Schedule' ? { kind: 'text' as const, text: 'scheduled' } : null,
      invocation?.kind === 'Event'
        ? {
            kind: 'chip' as const,
            name: invocation.event_type,
            onClick: onOpenMatched,
            pending: opening,
            role: 'trigger-event-jump',
          }
        : null,
    ],
    fold: event.prompt
      ? { label: 'Prompt', body: <MarkdownBlock html={renderMarkdown(event.prompt)} /> }
      : undefined,
  });
}

/** Diff/Revert action buttons rendered in the initiator panel's action slot
 *  for ChangeApplied/Discarded/Reverted exchanges. Returns null when the
 *  change has no relevant actions (e.g. ChangeApplyFailed leaves the change
 *  pending — user reads the error, doesn't diff/revert).
 *
 *  `reserveWhileLoading` (set for ChangeApplied panels, which always get at
 *  least a Revert button) renders a hidden button placeholder while the per-id
 *  `Change` row is still being fetched — so the real Diff/Revert buttons slot
 *  into an already-reserved footer row without a vertical shift on first open.
 *  Mirrors the body's `seed*` props in <ChangeBody>. */
export function changeActions(changeId?: string, suppress?: boolean, reserveWhileLoading?: boolean): ComponentChildren {
  if (suppress || !changeId) return null;
  const change = findChangeById(changeId);
  if (!change) {
    return reserveWhileLoading
      ? <button class="action-btn change-actions-placeholder" aria-hidden="true" tabIndex={-1}>Revert</button>
      : null;
  }
  const showDiff = change.status === 'pending' || !!change.pre_merge_sha;
  const showRevert = change.status === 'applied';
  if (!showDiff && !showRevert) return null;
  return (
    <>
      {showDiff && <button class="action-btn" onClick={() => void viewChangeDiff(change)}>Diff</button>}
      {showRevert && <button class="action-btn action-btn-danger" onClick={() => void revertChange(change.id)}>Revert</button>}
    </>
  );
}

/** Shared header click handler for InitiatorPanel/ResponsePanel — toggles the
 *  panel's collapsed state, but ignores clicks that originated on a button or
 *  link inside the header (actor info, executor info, stop). */
function handlePanelHeaderClick(e: MouseEvent, onToggle?: () => void): void {
  if (!onToggle) return;
  if ((e.target as HTMLElement).closest('button, a')) return;
  onToggle();
}

/** Shown in place of a collapsed panel's body (initiator OR response) so a
 *  folded turn never reads as an empty row — a muted "⋯", rendered as an
 *  accent stub bubble for user messages. Clicking it expands the panel. */
function CollapsedIndicator({ bubble = false, onToggle }: { bubble?: boolean; onToggle?: () => void }) {
  return (
    <div class={`turn-collapsed${bubble ? ' turn-collapsed-bubble' : ''}`} onClick={onToggle}>
      <span class="turn-collapsed-dots" aria-label="Collapsed — click to expand">⋯</span>
    </div>
  );
}

interface InitiatorPanelProps {
  initiator: InitiatorDescriptor;
  timestamp: string;
  onActorClick?: (e: MouseEvent) => void;
  actions?: ComponentChildren;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
  /** User message → render the body as a right-aligned gray bubble. */
  bubble?: boolean;
  /** Drop the actor chip (icon + name) entirely — used for user messages and
   *  change-lifecycle turns. Attribution is reached via the clickable timestamp
   *  (and, for change turns, the summary line), which open the route popover. */
  chromeless?: boolean;
}

function ActorChipBody({ initiator }: { initiator: InitiatorDescriptor }) {
  return (
    <>
      {/* Iconless chips (e.g. the "Response canceled" boundary header) skip the
          icon span entirely so the label sits flush, with no leading gap. */}
      {initiator.icon != null && initiator.icon !== '' && (
        <span class="initiator-icon">{initiator.icon}</span>
      )}
      <span class="initiator-label">{initiator.label}</span>
    </>
  );
}

export function InitiatorPanel({ initiator, timestamp, onActorClick, actions, collapsible, collapsed, onToggle, bubble = false, chromeless = false }: InitiatorPanelProps) {
  const accentClass = initiator.accent ? ` initiator-panel-${initiator.accent}` : '';
  const hasBody = !!initiator.summary || !!initiator.details;
  // A chromeless turn whose summary opens the popover renders that summary as a
  // button ("Change applied" → origin info). Plain summaries stay a <div>.
  const summaryLinks = chromeless && !!onActorClick;

  return (
    <div class={`initiator-panel initiator-panel-${initiator.variant}${accentClass}${bubble ? ' initiator-panel-bubble' : ''}${collapsed ? ' initiator-panel-collapsed' : ''}`}>
      <div
        class={`initiator-header${collapsible ? ' initiator-header-clickable' : ''}`}
        onClick={(e) => handlePanelHeaderClick(e, onToggle)}
      >
        {!chromeless && (onActorClick ? (
          <button
            type="button"
            class="initiator-actor"
            onClick={onActorClick}
            aria-label={`Show info for ${initiator.label}`}
          >
            <ActorChipBody initiator={initiator} />
          </button>
        ) : (
          <span class="initiator-actor">
            <ActorChipBody initiator={initiator} />
          </span>
        ))}
        {/* Status (question/permission resolution) + timestamp, grouped right.
            Chromeless turns have no actor chip — the timestamp is the popover
            trigger; it's a <button> so it's excluded from the collapse click. */}
        <span class="initiator-meta">
          {initiator.status}
          {chromeless && onActorClick ? (
            <button
              type="button"
              class="initiator-timestamp initiator-timestamp-button"
              onClick={onActorClick}
              aria-label={`Show info for ${initiator.label}`}
            >
              {timestamp}
            </button>
          ) : (
            <span class="initiator-timestamp">{timestamp}</span>
          )}
        </span>
      </div>
      {hasBody && !collapsed && (
        <div class="initiator-body">
          {initiator.summary && (summaryLinks ? (
            <button type="button" class="initiator-summary initiator-summary-link" onClick={onActorClick}>
              {initiator.summary}
            </button>
          ) : (
            <div class="initiator-summary">{initiator.summary}</div>
          ))}
          {bubble ? <div class="user-bubble">{initiator.details}</div> : initiator.details}
        </div>
      )}
      {hasBody && collapsed && <CollapsedIndicator bubble={bubble} onToggle={onToggle} />}
      {actions && !collapsed && <div class="initiator-footer">{actions}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Response panel — bordered card wrapping the executor's reply. Header carries
// executor + status + (optional) stop button + timestamp + collapse toggle.
// Body is the response content; collapses when the user clicks the header.
// ---------------------------------------------------------------------------

/** Triggers invoke the Lucidos agent rather than running their own executor,
 *  so the label always reflects the LLM that produced the response. The model
 *  is shown in the executor info popover, not in the header. The "Lucidos
 *  Agent" name matches the initiator label used when one Lucidos thread
 *  spawns another — same entity, same display. */
/** Claude Code reads "Claude" HERE and nowhere else. This row is the tightest in
 *  the app on a phone (the executor, then three turn controls, then the status,
 *  then the timestamp) and the coding agent's own name was the longest thing in
 *  it. Every other "Claude Code" in the app names the BACKEND the user is
 *  choosing between (the compose destination, the settings binaries row, the
 *  coding-agent control menu, the drawer's channel tag), and there the full
 *  product name is the point, so those stay. The icon beside it carries the rest
 *  of the identity, and the popover the label opens names the model exactly. */
export function describeExecutor(
  isCC: boolean,
  codingAgent: CodingAgent = 'claude-code',
): { icon: ComponentChildren; label: string } {
  if (isCC && codingAgent === 'codex') return { icon: <CodexIcon />, label: 'Codex' };
  if (isCC) return { icon: <ClaudeIcon />, label: 'Claude' };
  return { icon: <LucidosGlyph />, label: LUCIDOS_AGENT_LABEL };
}

interface TurnControlsProps {
  /** Current state of each toggle, which is what the icon and `aria-pressed` say. */
  detailsOn: boolean;
  stepsOn: boolean;
  /** This turn folded to its `⋯` stub. Unlike the two above, per-turn state. */
  collapsed: boolean;
  /** Whether this turn HAS a body to fold (`canCollapse`). False on a panel
   *  that is only a status line so far, and the collapse control is disabled
   *  there: the store would take the fold and hold it, so the click would look
   *  dead and then land later, folding the turn the moment its first content
   *  arrived. */
  collapsible: boolean;
  onToggleDetails: () => void;
  onToggleSteps: () => void;
  onToggleCollapsed: () => void;
}

/** The response header's `controls` slot: three icon buttons right of the
 *  executor label, evenly spaced as one run.
 *
 *  The first two are the detail toggles, the full response (the prose the
 *  default view drops) and the step log. They were text links at the top of the
 *  response BODY ("More" / "Less" and "Show steps" / "Hide steps"), which
 *  pushed the answer down by a row and reflowed that row on every click, since
 *  both labels change width as they toggle. As icons in the header they cost
 *  the body nothing and the row's width never moves.
 *
 *  Both always render, whatever this particular turn happens to hold. What they
 *  flip is a per-user setting that spans the whole transcript, so a turn with
 *  no steps of its own is not a turn where "show steps" means nothing: the
 *  reader is setting how turns read, from wherever they are looking. The links
 *  were conditional, and that made the pair jump around from turn to turn and
 *  left holes in a column of otherwise identical headers.
 *
 *  The third folds THIS turn to its `⋯` stub, and it is the only one whose
 *  effect stops at the turn it sits on. Its LABEL is what says so, naming the
 *  turn where the other two name what they reveal. An extra 0.3125rem of gap
 *  used to say it too, and that is gone (styles/chat/response.css): a run of
 *  three icons broken 2+1 makes a claim the eye reads before any tooltip, and
 *  it lands as "two things and a stray" rather than as a scope split. It used
 *  to be a click anywhere on the header row, which nothing announced; the row
 *  is inert now and this is the affordance.
 *
 *  It is also the only one that states its state in its GLYPH rather than its
 *  brightness: the arrowheads point together to fold and apart to unfold, and
 *  the brightness rule skips it. Both halves of that are the scope split again.
 *  Bright means "on, you are seeing MORE" on the pair, so bright-means-FOLDED
 *  would be the same cue inverted 0.125rem away from the two it contradicts,
 *  and it would report a state the `⋯` stub below is already unmissable about.
 *  The pair keeps a fixed glyph for the reason `FullResponseIcon` records, and
 *  this is a deliberate EXCEPTION to that reason, not a case it fails to reach.
 *  The line above used to claim the latter, on the grounds that the two forms
 *  here are one mark reflected; the banned mark was built that way too, so the
 *  geometry separates nothing and the argument was false. What separates them
 *  is the payoff: there, `aria-pressed` and brightness already answered "is it
 *  on", so the movement bought nothing. Here brightness is gone, which leaves
 *  direction as the only state cue there is. `CollapseTurnIcon` carries the
 *  long form.
 *
 *  Each is a `<button>`, which is what keeps a click off the initiator header's
 *  collapse handler: `handlePanelHeaderClick` ignores anything inside a
 *  `button, a`. */
export function turnControls({
  detailsOn, stepsOn, collapsed, collapsible, onToggleDetails, onToggleSteps, onToggleCollapsed,
}: TurnControlsProps): ComponentChildren {
  // "the LATEST answer" and not "the answer": turning the full response off keeps
  // only what follows the last text block (`getCollapsedVisibleEvents`), so a turn
  // that said something, worked, and then said something else loses the first of
  // the two. The old wording promised one answer where there had been several,
  // and read as though the turn had only ever given one.
  const detailsLabel = detailsOn ? 'Show the latest answer only' : 'Show the full response';
  const stepsLabel = stepsOn ? 'Hide steps' : 'Show steps';
  const collapseLabel = collapsed ? 'Expand this turn' : 'Collapse this turn';
  return (
    <span class="response-controls">
      <button
        type="button"
        class="icon-btn"
        data-role="toggle-details"
        aria-pressed={detailsOn}
        aria-label={detailsLabel}
        data-tooltip={detailsLabel}
        onClick={onToggleDetails}
      >
        <FullResponseIcon />
      </button>
      <button
        type="button"
        class="icon-btn"
        data-role="toggle-steps"
        aria-pressed={stepsOn}
        aria-label={stepsLabel}
        data-tooltip={stepsLabel}
        onClick={onToggleSteps}
      >
        <StepLogIcon />
      </button>
      <button
        type="button"
        class="icon-btn response-control-turn"
        data-role="toggle-collapsed"
        disabled={!collapsible}
        aria-pressed={collapsed}
        aria-label={collapseLabel}
        data-tooltip={collapseLabel}
        onClick={onToggleCollapsed}
      >
        <CollapseTurnIcon collapsed={collapsed} />
      </button>
    </span>
  );
}

interface ResponsePanelProps {
  executor: { icon: ComponentChildren; label: string };
  onExecutorClick?: (e: MouseEvent) => void;
  /** The turn's controls (`turnControls`), rendered immediately right of the
   *  executor label. They render in every state, collapsed included: the
   *  collapse control is one of them, so hiding the group would fold a turn
   *  and take away the thing that unfolds it. */
  controls?: ComponentChildren;
  status: ComponentChildren;
  timestamp: string;
  collapsed: boolean;
  onToggle?: () => void;
  hasBody: boolean;
  children: ComponentChildren;
}

/** The response half of a turn.
 *
 *  Its header is NOT a click target, unlike the initiator panel's: folding this
 *  turn is the third turn control's job. A whole row that silently swallowed a
 *  click announced nothing, and it sat under three buttons that each mean
 *  something else. */
export function ResponsePanel({
  executor, onExecutorClick, controls, status, timestamp, collapsed, onToggle, hasBody, children,
}: ResponsePanelProps) {
  return (
    <div class={`response-panel${collapsed ? ' response-panel-collapsed' : ''}${hasBody ? '' : ' response-panel-bodyless'}`}>
      <div class="response-header">
        <button
          type="button"
          class="response-executor"
          onClick={onExecutorClick}
          aria-label="Show executor info"
        >
          <span class="response-executor-icon">{executor.icon}</span>
          <span class="response-executor-label">{executor.label}</span>
        </button>
        {controls}
        <span class="response-meta">
          {status}
          <span class="response-timestamp">{timestamp}</span>
        </span>
      </div>
      {hasBody && !collapsed && (
        <div class="response-body">
          {children}
        </div>
      )}
      {hasBody && collapsed && <CollapsedIndicator onToggle={onToggle} />}
    </div>
  );
}

/** Cap a preview string at `max` characters, marking the cut with an ellipsis.
 *  The one place the response's preview-truncation convention lives, so the
 *  step ticker and the image tooltip clip the same way. */
function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/** Longest prompt we surface as an image tooltip. A generation prompt runs to
 *  several paragraphs routinely, and a tooltip that fills the pane is worse
 *  than a short one; the untruncated prompt stays one click away in the
 *  generating step's detail modal. */
const IMAGE_PROMPT_SUMMARY_MAX = 240;

/** A generated image's description: the prompt it came from, flattened to one
 *  line and capped. Undefined when no prompt was recorded, which is what makes
 *  the image carry NO tooltip rather than a meaningless one. */
export function imagePromptSummary(prompt: string | undefined, max = IMAGE_PROMPT_SUMMARY_MAX): string | undefined {
  const text = prompt?.replace(/\s+/g, ' ').trim();
  if (!text) return undefined;
  return clip(text, max);
}

/** Generated image rendered inline in the response.
 *
 *  The tooltip lives on the `<img>`, not on the block wrapper: the wrapper is
 *  as wide as the response column, so anchoring there centered the tooltip
 *  over empty space beside the picture. */
export function GeneratedImage({ event }: { event: Extract<ResponseEvent, { type: 'image' }> }) {
  const src = `data:${event.mime_type};base64,${event.base64}`;
  const summary = imagePromptSummary(event.prompt);
  return (
    <div class="generated-image">
      <img
        src={src}
        class="image-thumbnail"
        alt={summary ?? 'Generated image'}
        data-tooltip={summary}
        loading="lazy"
      />
    </div>
  );
}

/** Latest non-empty line of streamed reasoning, truncated — the inline "ticker"
 *  preview for a live Thinking step. The full text lives in the detail modal. */
function lastLinePreview(text: string, max = 120): string {
  const line = text.split('\n').map(s => s.trim()).filter(Boolean).pop() ?? '';
  return clip(line, max);
}

/** The context-usage suffix on a step row.
 *
 *  `compact` keeps only the percentage. The full "178k / 1000k (18%)" is wider
 *  than a narrow row's remaining space, and since the row is `nowrap` the excess
 *  would push the description out rather than wrap. The percentage is the part
 *  that is read at a glance anyway.
 *
 *  Which form the user sees is decided in CSS, not here: `InlineStep` renders
 *  both and a container query on the row picks one (see `.step-context-compact`
 *  in steps.css). */
export function contextLabel(
  used: number,
  window: number | null | undefined,
  messages: number | null | undefined,
  compact: boolean,
): string {
  if (window) {
    const pct = `${contextPercent(used, window)}%`;
    return compact ? pct : `${formatTokens(used)} / ${formatTokens(window)} (${pct})`;
  }
  // No window to divide by, so there is no percentage: the token count alone is
  // the compact form.
  if (compact) return formatTokens(used);
  return `${formatTokens(used)} tokens${messages != null ? `, ${messages} msgs` : ''}`;
}

/** One action, as one row: the model's thinking and the call it produced share
 *  a row rather than taking two (see `nameThinkingRow` in the projection).
 *
 *  The row therefore has TWO click targets, which is why it is a `<div>` around
 *  two buttons rather than one button (a `<button>` may not contain another
 *  interactive element). The main target opens what the step DID; the context
 *  counter opens what the model was SENT. */
export function InlineStep({ event }: { event: Extract<ResponseEvent, { type: 'step' }> }) {
  const { label, icon, className } = stepStatus(event.outcome);
  const snap = event.contextCapture;
  const used = snap?.usage?.input_tokens ?? snap?.estimated_total_tokens ?? event.context_tokens;
  const window = snap?.context_window;
  const trimmed = snap?.trimmed ?? event.trimmed;
  const hasContext = used != null;
  // Reasoning streams into `thinkingText` while the row is still an unnamed
  // "Thinking" marker. Show its tail (latest non-empty line, truncated) as a
  // live ticker so a long reasoning pass visibly progresses. It stops at the
  // rename: once the row names the call it produced, a truncated mid-sentence
  // fragment trailing "Running: cd …" is noise, and the full reasoning is one
  // click away in the step detail.
  const thinkingTail = isThinking(event) && event.thinkingText
    ? lastLinePreview(event.thinkingText)
    : '';
  const detailText = event.detail || thinkingTail;

  const isPending = event.outcome === 'pending';
  // Both forms go into the DOM and CSS shows exactly one, because what decides
  // is the ROW's own width, which nothing here can read: the row is as wide as
  // the thread pane the user dragged the divider to, so a narrow pane on a
  // desktop crowds the counter exactly the way a phone does. The gate is a
  // container query on `.inline-step` (steps.css). Measuring in JS instead would
  // mean re-rendering every step row on every frame of that drag.
  const counter = hasContext && (
    <>
      <span class="step-context-full">
        {contextLabel(used!, window, event.context_messages, false)}
      </span>
      <span class="step-context-compact">
        {contextLabel(used!, window, event.context_messages, true)}
      </span>
      {trimmed && ' · trimmed'}
    </>
  );

  return (
    <div
      class={`inline-step ${className}`}
      data-role="inline-step"
      /* A row the user can't read at a glance needs naming: a killed-mid-call
         step is struck and muted, and the tooltip says what that means without
         a trip through the detail modal. */
      data-tooltip={event.outcome === 'unfinished' ? label : undefined}
    >
      <button
        type="button"
        class="step-main"
        data-role="step-main"
        onClick={() => { stepDetailModal.value = event; }}
      >
        {/* In-progress step: no leading mark (`stepStatus` returns an empty
            icon), because the shimmering description is the "live" affordance.
            The span still renders, empty: the slot is a fixed-width column in
            CSS, so the running row's text sits on the same column as the
            finished rows above it. */}
        <span class="step-icon">{icon || null}</span>
        <span class={`step-description${isPending ? ' running-shimmer' : ''}`}>{highlightEllipsis(event.description)}</span>
        {detailText && <span class="step-detail">{highlightEllipsis(detailText)}</span>}
      </button>
      {/* The counter is a button only when there is a snapshot behind it. A
          legacy row carries `context_tokens` with nothing to open, and a button
          that opens nothing is worse than plain text. */}
      {counter && (snap ? (
        <button
          type="button"
          class={`step-context${trimmed ? ' trimmed' : ''}`}
          data-role="step-context"
          aria-label="Show the context sent for this call"
          onClick={() => { contextViewer.value = { snapshot: snap, description: event.description }; }}
        >
          {counter}
        </button>
      ) : (
        <span class={`step-context${trimmed ? ' trimmed' : ''}`}>{counter}</span>
      ))}
    </div>
  );
}

type EventWaitState = Extract<ResponseEvent, { type: 'event_wait' }>['state'];

/** How each state reads on the row: the mark, and the state WORD with its tone.
 *
 *  None of these is a step outcome, and that is the point. The row rendered
 *  through `stepStatus` until 2026-08-10, which mapped `waiting` to `success`
 *  and therefore painted a green check on a subscription that might sleep for
 *  hours. A marker reports a fact; only the child-thread row legitimately shows
 *  a verdict, and that verdict is the CHILD's.
 *
 *  `waiting` reads as live rather than in-progress: a shimmer would run for
 *  however long the thread sleeps and claim a turn was running when none is.
 *  `timed_out` and `canceled` are told apart by their words, not by red:
 *  nothing failed either time, the watch simply ended without its event. */
const EVENT_WAIT_ROW_STATE: Record<
  EventWaitState,
  { mark: EventRowMark; label: string; tone: EventRowTone }
> = {
  waiting: { mark: 'pending', label: 'waiting', tone: 'live' },
  woke: { mark: 'arrived', label: 'woke', tone: 'arrived' },
  timed_out: { mark: 'pending', label: 'timed out', tone: 'lapsed' },
  canceled: { mark: 'pending', label: 'stopped', tone: 'halted' },
};

/** Who stopped a subscription, in the words of whatever they pressed.
 *
 *  The word is **stopped**, never "discarded": *discarded* already means
 *  throwing a thing away in Lucidos (a discarded thread, change or draft), and
 *  one of these causes literally IS a discarded thread, so reusing it here
 *  would collide. "Stop waiting" is what the button says, so a stop is what
 *  this reads as. (The `canceled` identifiers underneath keep their names: they
 *  are on disk in persisted rows.)
 *
 *  A pre-2026-08-07 row carries no cause and falls back to the bare note, which
 *  says it stopped without claiming to know how. */
const EVENT_WAIT_STOP_NOTE: Record<EventWaitCancelCause, string> = {
  user_stop: 'stopped from the panel',
  agent_stand_down: 'stood down',
  thread_archived: 'stopped by archiving',
  thread_discarded: 'stopped by discarding the thread',
  // Retired: a thread-level Stop no longer stops a subscription. Only old rows
  // carry it, and they still have to render.
  thread_canceled: 'stopped by a thread Stop',
  unknown: 'stopped',
};

/** An event wait, as an **event row** (ADR 0047, amended 2026-08-06 and
 *  2026-08-10).
 *
 *  It was a boxed `.step-note-card` until the box was dropped as too heavy for
 *  one action with no affordance in it, and then an `.inline-step` until that
 *  turned out to be the opposite mistake: it read as a finished tool call,
 *  green check and all, and ellipsized the reason and the subscription, which
 *  are the only two things the row has to say. It now shares one marker with
 *  the event wake, the child callback and the trigger fire.
 *
 *  Still deliberately NOT an exchange divider: an attached delivery resumes the
 *  SAME exchange, so the wake's steps continue below this row rather than under
 *  a fresh boundary.
 *
 *  It holds no hook and no affordance. The jump to the matched event lived here
 *  until 2026-08-10 and moved to the wake card below, where the event actually
 *  arrived: this card is about the ARMING, and a link out of it to something
 *  that happened hours later did not read as belonging to it. */
export function EventWaitRow({ event }: { event: Extract<ResponseEvent, { type: 'event_wait' }> }) {
  return eventWaitRowBody({ event });
}

/** The row's markup, hookless so it stays a pure function of its state (same
 *  split as `eventWaitIndicatorBody` next door). The component above owns the
 *  one hook, which is also why the split exists: there is no jsdom in the test
 *  infra, so a component carrying a hook cannot be invoked as a plain function
 *  and the tests drive this instead. */
export function eventWaitRowBody({
  event,
}: {
  event: Extract<ResponseEvent, { type: 'event_wait' }>;
}) {
  const { mark, label, tone } = EVENT_WAIT_ROW_STATE[event.state];
  const stopped = event.state === 'canceled';
  // A stop is a different action from an arming and says so. The wording comes
  // from `eventWaitStoppedSummary` rather than being spelled again here, because
  // the user's own stop renders as a TURN with that same header (see
  // `describeInitiator`) and one concept must not acquire two phrasings.
  // It also handles the reason-less row: a pre-2026-08-07 `EventWaitCanceled`
  // carries no copy of what it stopped, so the line says the one thing it does
  // know rather than trailing an empty colon.
  //
  // "event wait" is the canonical term in both glossaries, and the indicator
  // beside this already reads "Watching for an event". "event tracking" was the
  // phrasing this line was sketched with and is a synonym for the same concept,
  // which is exactly the drift `.claude/rules/glossary.md` exists to stop.
  //
  // A colon rather than "for", because `reason` is the model's own words and it
  // reaches for a gerund as often as a noun phrase: "…an event wait for Watching
  // for the next code edit" is what the preposition produced on a real thread.
  // The colon reads as an introduction either way.
  const subject = stopped
    ? eventWaitStoppedSummary(event.reason)
    : `Set up an event wait: ${event.reason}`;
  const deadline = event.state === 'waiting' ? waitDeadline(event.expires_at) : undefined;
  return eventRowBody({
    kind: 'wait',
    mark,
    state: event.state,
    role: 'event-wait-row',
    subject,
    stateLabel: label,
    tone,
    facts: [
      // A stop names how it ended instead of what it watched: the subscription
      // is over, and how it ended is the new fact. A pre-2026-08-07 row knows
      // neither cause nor types, and then says only that it stopped.
      stopped && event.cause ? { kind: 'text' as const, text: EVENT_WAIT_STOP_NOTE[event.cause] } : null,
      // The matched event REPLACES the subscription list on a wake: one of the
      // types it was watching for is now a specific thing that happened.
      ...(event.state === 'woke'
        ? [event.matched_event_type ? { kind: 'chip' as const, name: event.matched_event_type } : null]
        : stopped
          ? []
          : subscriptionFacts(event.subscriptions)),
      deadline ? { kind: 'text' as const, text: deadline } : null,
      // **No jump from here.** This card records the ARMING: an action the agent
      // took, at the moment it took it. A link out to the event that eventually
      // matched reads as belonging to a card about that event, and there is one,
      // directly below (the wake, `EventDeliveryBody`), which is where the jump
      // now lives. Naming the matched type here is still right, since it says
      // how this wait ended.
    ],
  });
}

/** The watched event types as chips, joined by the word the subscription
 *  language itself uses. "or" is `glue` rather than a fact, so the row's middot
 *  separator steps over it: three items, one fact. */
function subscriptionFacts(subscriptions: string[]): EventRowFact[] {
  return subscriptions.flatMap((name, i) =>
    i === 0
      ? [{ kind: 'chip' as const, name }]
      : [{ kind: 'glue' as const, text: 'or' }, { kind: 'chip' as const, name }],
  );
}

/** When an unresolved wait gives up, as a fact rather than a countdown.
 *
 *  Deliberately not ticking. ADR 0047's amendment puts the live countdown on the
 *  subscription indicator and says the transcript record gets LIGHTER, and a
 *  per-second span here would re-render inside `ChatExchange`, which is the
 *  component the whole store is shaped around not re-rendering. The panel's own
 *  countdown keeps its interval because it is one open popover, not one row per
 *  wait in a scrollback.
 *
 *  **The day is named whenever the deadline is not today.** An `await_event`
 *  timeout runs up to 24 hours, so a deadline is routinely tomorrow, and a bare
 *  "until 09:15" read at 14:00 points at a time that already passed this
 *  morning. Same-day is compared in the user's configured timezone, which is
 *  what `isSameDayInUserTz` exists for.
 *
 *  Absent when the deadline is unparseable or missing, rather than rendering an
 *  "Invalid Date": a row states no fact its event does not carry. */
function waitDeadline(expiresAt: string): string | undefined {
  if (!expiresAt) return undefined;
  const at = new Date(expiresAt);
  if (Number.isNaN(at.getTime())) return undefined;
  return isSameDayInUserTz(at, new Date())
    ? `until ${formatShortTime(at)}`
    : `until ${formatShortDate(at)} ${formatShortTime(at)}`;
}

/** What Undo will do to the workspace, in words, from the counts the engine
 *  recorded when it diffed the checkpoint's two snapshots.
 *
 *  `null` when both are 0, which is a checkpoint written before the counts
 *  existed rather than one that changed nothing (a command that changed nothing
 *  git-visible emits no card at all). Saying "restore 0 files" there would
 *  invent a fact; saying nothing leaves the diff as the way to find out. */
export function checkpointUndoScope(restores: number, removes: number): string | null {
  const parts: string[] = [];
  if (restores > 0) parts.push(`restore ${formatFileCount(restores)}`);
  if (removes > 0) parts.push(`remove ${formatFileCount(removes)} this step created`);
  return parts.length > 0 ? `Undo will ${parts.join(' and ')}.` : null;
}

/** The command-guard checkpoint card (ADR 0002, Phase 4): an inline affordance
 *  for an in-workspace destructive command, with a one-click Undo that restores
 *  the workspace from the snapshot taken before it and removes the files it
 *  created. Diff opens the change between those two snapshots, so the Undo
 *  beside it is not a blind button, and it is the same `.action-btn` Diff the
 *  changes list and the change banners carry rather than a one-off link. Once
 *  reverted (live via the paired `CommandCheckpointReverted` event, or on
 *  reload), Undo is replaced with a reverted marker, while the diff stays
 *  available. */
export function CheckpointCard({ event }: { event: Extract<ResponseEvent, { type: 'checkpoint' }> }) {
  const pending = useSignal(false);
  const onUndo = async () => {
    if (pending.value || event.reverted) return;
    pending.value = true;
    try {
      // Success flips `reverted` via the SSE CommandCheckpointReverted event,
      // which re-groups into this exchange and re-renders the card.
      await postCommandCheckpointUndo(event.checkpoint_id);
    } catch (e) {
      showToast('Undo failed: ' + errorDetail(e), 'error');
    } finally {
      pending.value = false;
    }
  };
  const scope = checkpointUndoScope(event.restores, event.removes);
  return (
    <div class="step-note-card checkpoint-card" data-role="checkpoint-card">
      <span class="step-note-icon" aria-hidden="true">{'⎌'}</span>
      <div class="step-note-body">
        <div class="step-note-summary">{event.summary}</div>
        <code class="step-note-detail">{event.command}</code>
        {scope && <div class="checkpoint-scope">{scope}</div>}
      </div>
      <div class="checkpoint-actions">
        <button
          type="button"
          class="action-btn"
          data-role="checkpoint-diff"
          onClick={() => { checkpointDiffModal.value = event; }}
        >
          Diff
        </button>
        {event.reverted ? (
          <span class="checkpoint-reverted">Reverted ✓</span>
        ) : (
          <button
            type="button"
            class="action-btn action-btn-danger"
            data-role="checkpoint-undo"
            disabled={pending.value}
            onClick={onUndo}
          >
            {pending.value ? 'Undoing…' : 'Undo'}
          </button>
        )}
      </div>
    </div>
  );
}
