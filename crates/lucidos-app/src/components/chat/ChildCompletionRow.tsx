import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { threadMap } from '../../store/store';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { eventNameChip, eventRowBody } from './EventRow';
import type { EventRowTone } from './EventRow';
import type { ChildCompletionStatus, SubThreadPendingChange } from '../../store/thread-events';

interface Props {
  childThreadId: string;
  childThreadTitle?: string;
  status: ChildCompletionStatus;
  summary: string;
  /** Changes the child left pending, if it left any. Absent on a row written
   *  before the field existed, which is why the count is omitted rather than
   *  rendered as zero: a row states no fact its event does not carry. */
  pendingChangeIds?: string[];
  /** Changes pending anywhere below the child, kept apart from its own. An
   *  orchestrator's children hold the work while it holds none. */
  subThreadPendingChanges?: SubThreadPendingChange[];
  /** Set while the report waits behind an open question (`EventRowProps`). */
  heldNote?: string;
}

/** How each completion reads on the row. This is the one event row that
 *  legitimately shows a pass or a fail, because the verdict it reports is the
 *  CHILD's outcome rather than the row's own.
 *
 *  They appear together in one stream, so each word has to be distinguishable
 *  from the others. `canceled` is warm rather than the cool neutral, so it is
 *  not a near-twin of the untinted `no changes` word beside it. `interrupted`
 *  shares the warm tone: the child stopped short, and its word tells why. */
const CHILD_STATE: Record<ChildCompletionStatus, { label: string; tone: EventRowTone }> = {
  success: { label: 'Success', tone: 'good' },
  failure: { label: 'Failure', tone: 'bad' },
  no_changes: { label: 'No changes', tone: 'none' },
  canceled: { label: 'Canceled', tone: 'halted' },
  interrupted: { label: 'Interrupted', tone: 'halted' },
};

/** A link that opens `threadId`, labelled with its title. Routed through
 *  `focusThreadOrBootstrap` so a thread outside the loaded window still opens. */
export function threadLink(threadId: string, title: string | undefined | null) {
  return (
    <button
      type="button"
      class="accent-link"
      onClick={() => focusThreadOrBootstrap(threadId)}
      data-thread-id={threadId}
    >
      {title?.trim() || 'Untitled thread'}
    </button>
  );
}

/** A child thread reporting back to its parent, as an **event row**. It wears
 *  the card an event wait, an event wake and a trigger fire use. All of them say
 *  one thing: something happened outside this thread. See
 *  `docs/plans/2026-08-10-one-event-row-for-the-transcript.md`.
 *
 *  Flat, with no chrome of its own: the surrounding `InitiatorPanel` owns that.
 *  The title link is the row's origin affordance, which is why the panel's actor
 *  chip is not clickable (see the `ChildThreadCompleted` arm of
 *  `describeInitiator`). */
export function ChildCompletionRow(props: Props) {
  const { label, tone } = CHILD_STATE[props.status];
  const summaryHtml = props.summary.trim() ? renderMarkdown(props.summary) : '';
  const pending = props.pendingChangeIds?.length ?? 0;
  const below = props.subThreadPendingChanges?.length ?? 0;
  return eventRowBody({
    kind: 'child',
    state: props.status,
    role: 'child-completion',
    subject: eventNameChip({ kind: 'chip', name: 'ChildThreadCompleted', sentenceStart: true }),
    stateLabel: label,
    tone,
    heldNote: props.heldNote,
    facts: [
      { kind: 'node' as const, node: threadLink(props.childThreadId, props.childThreadTitle) },
      pending > 0
        ? { kind: 'text' as const, text: `${pending} pending change${pending === 1 ? '' : 's'}` }
        : null,
      below > 0
        ? {
            kind: 'text' as const,
            text: `${below} pending in its sub-threads`,
          }
        : null,
    ],
    fold: summaryHtml
      ? {
          label: 'Details',
          body: (
            <div class="markdown-content" dangerouslySetInnerHTML={{ __html: summaryHtml }} />
          ),
        }
      : undefined,
  });
}

interface StoppedProps {
  childThreadId: string;
  childThreadTitle?: string;
}

/** What a stopped row's state says, read from the child's live meta. The event
 *  is immutable, but "Waiting for you" is a present-tense claim, so it holds
 *  only while the child is still stopped. An unloaded child has no live state,
 *  and the row then says only what the event proves. */
function childStoppedState(childThreadId: string): { label: string; tone: EventRowTone } {
  const child = threadMap.value.get(childThreadId);
  if (!child) return { label: 'Stopped', tone: 'halted' };
  return child.meta.isStoppedChild
    ? { label: 'Waiting for you', tone: 'live' }
    : { label: 'No longer waiting', tone: 'none' };
}

/** A user Stop paused one of this thread's children, as an event row
 *  (ADR 0252). It is the parent-side half of a *stopped child*: the child is
 *  alive and waits for the user, and this thread was not woken. So the state
 *  says who the child is waiting for, until the child moves on. */
export function ChildStoppedRow(props: StoppedProps) {
  const { label, tone } = childStoppedState(props.childThreadId);
  return eventRowBody({
    kind: 'child',
    state: 'stopped',
    role: 'child-stopped',
    subject: eventNameChip({ kind: 'chip', name: 'ChildThreadStopped', sentenceStart: true }),
    stateLabel: label,
    tone,
    facts: [{ kind: 'node', node: threadLink(props.childThreadId, props.childThreadTitle) }],
  });
}

/** One of this thread's children was moved to top level (ADR 0278). It is no
 *  longer this thread's child and will send it nothing, so the state says
 *  nothing is coming. */
export function ChildMovedOutRow(props: StoppedProps) {
  return eventRowBody({
    kind: 'child',
    state: 'moved-out',
    role: 'child-moved-out',
    subject: eventNameChip({ kind: 'chip', name: 'ChildThreadDetached', sentenceStart: true }),
    facts: [{ kind: 'node', node: threadLink(props.childThreadId, props.childThreadTitle) }],
    stateLabel: 'No longer waiting',
    tone: 'none',
  });
}
