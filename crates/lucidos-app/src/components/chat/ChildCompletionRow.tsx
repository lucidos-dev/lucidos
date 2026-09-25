import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { eventRowBody } from './EventRow';
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
}

/** How each completion reads on the row. This is the one event row that
 *  legitimately shows a pass or a fail, because the verdict it reports is the
 *  CHILD's outcome rather than the row's own.
 *
 *  The four appear together in one stream, so each has to be distinguishable
 *  from the other three. `canceled` is warm rather than the cool neutral, so it
 *  is not a near-twin of the untinted `no changes` pill beside it. */
const CHILD_STATE: Record<ChildCompletionStatus, { verb: string; label: string; tone: EventRowTone }> = {
  success: { verb: 'returned', label: 'success', tone: 'good' },
  failure: { verb: 'failed', label: 'failure', tone: 'bad' },
  no_changes: { verb: 'returned', label: 'no changes', tone: 'none' },
  canceled: { verb: 'canceled', label: 'canceled', tone: 'halted' },
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
 *  the marker an event wait, an event wake and a trigger fire use. All of them say
 *  one thing: something happened outside this thread. See
 *  `docs/plans/2026-08-10-one-event-row-for-the-transcript.md`.
 *
 *  Flat, with no chrome of its own: the surrounding `InitiatorPanel` owns that.
 *  The title link is the row's origin affordance, which is why the panel's actor
 *  chip is not clickable (see the `ChildThreadCompleted` arm of
 *  `describeInitiator`). */
export function ChildCompletionRow(props: Props) {
  const { verb, label, tone } = CHILD_STATE[props.status];
  const summaryHtml = props.summary.trim() ? renderMarkdown(props.summary) : '';
  const pending = props.pendingChangeIds?.length ?? 0;
  const below = props.subThreadPendingChanges?.length ?? 0;
  return eventRowBody({
    kind: 'child',
    mark: 'returned',
    state: props.status,
    role: 'child-completion',
    subject: (
      <>
        {`Child thread ${verb}: `}
        {threadLink(props.childThreadId, props.childThreadTitle)}
      </>
    ),
    stateLabel: label,
    tone,
    facts: [
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
          label: 'Summary',
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

/** A user Stop paused one of this thread's children, as an event row
 *  (ADR 0252). It is the parent-side half of a *stopped child*: the child is
 *  alive and waits for the user, and this thread was not woken. So the mark is
 *  the pending one, and the state says who the child is waiting for. */
export function ChildStoppedRow(props: StoppedProps) {
  return eventRowBody({
    kind: 'child',
    mark: 'pending',
    state: 'stopped',
    role: 'child-stopped',
    subject: (
      <>
        {'Child thread stopped: '}
        {threadLink(props.childThreadId, props.childThreadTitle)}
      </>
    ),
    stateLabel: 'waiting for you',
    tone: 'halted',
  });
}

/** One of this thread's children was moved to top level (ADR 0278). It is no
 *  longer this thread's child and will send it nothing, so the mark says
 *  nothing is coming. */
export function ChildMovedOutRow(props: StoppedProps) {
  return eventRowBody({
    kind: 'child',
    mark: 'returned',
    state: 'moved-out',
    role: 'child-moved-out',
    subject: (
      <>
        {'Child thread moved to top level: '}
        {threadLink(props.childThreadId, props.childThreadTitle)}
      </>
    ),
    stateLabel: 'no longer waiting',
    tone: 'none',
  });
}
