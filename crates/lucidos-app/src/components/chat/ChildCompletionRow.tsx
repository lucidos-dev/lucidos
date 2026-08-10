import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { eventRowBody } from './EventRow';
import type { EventRowTone } from './EventRow';
import type { ChildCompletionStatus } from '../../store/thread-events';

interface Props {
  childThreadId: string;
  childThreadTitle?: string;
  status: ChildCompletionStatus;
  summary: string;
  /** Changes the child left pending, if it left any. Absent on a row written
   *  before the field existed, which is why the count is omitted rather than
   *  rendered as zero: a row states no fact its event does not carry. */
  pendingChangeIds?: string[];
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

/** A child thread reporting back to its parent, as an **event row**: the same
 *  marker an event wait, an event wake and a trigger fire use.
 *
 *  It was its own card with its own prefix vocabulary, its own status pills and
 *  its own "Show summary" disclosure, which made three different dialects out of
 *  what is one concept: something happened outside this thread. See
 *  `docs/plans/2026-08-10-one-event-row-for-the-transcript.md`.
 *
 *  Flat, with no chrome of its own: the surrounding `InitiatorPanel` owns that.
 *  The title link is the row's origin affordance, which is why the panel's actor
 *  chip is not clickable (see the `ChildThreadCompleted` arm of
 *  `describeInitiator`). */
export function ChildCompletionRow(props: Props) {
  const { verb, label, tone } = CHILD_STATE[props.status];
  const titleText = props.childThreadTitle?.trim() || 'Untitled thread';
  const summaryHtml = props.summary.trim() ? renderMarkdown(props.summary) : '';
  const pending = props.pendingChangeIds?.length ?? 0;
  return eventRowBody({
    kind: 'child',
    mark: 'returned',
    state: props.status,
    role: 'child-completion',
    subject: (
      <>
        {`Child thread ${verb}: `}
        <button
          type="button"
          class="accent-link"
          onClick={() => focusThreadOrBootstrap(props.childThreadId)}
          data-thread-id={props.childThreadId}
        >
          {titleText}
        </button>
      </>
    ),
    stateLabel: label,
    tone,
    facts: [
      pending > 0
        ? { kind: 'text' as const, text: `${pending} pending change${pending === 1 ? '' : 's'}` }
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
