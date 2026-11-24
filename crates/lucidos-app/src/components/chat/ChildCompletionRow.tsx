import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { effectiveThreadStatus, isMidTurn, threadMap } from '../../store/store';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { eventNameChip, eventRowBody } from './EventRow';
import type { EventRowTone } from './EventRow';
import type { ChildCompletionStatus, SubThreadPendingChange } from '../../store/thread-events';

interface Props {
  childThreadId: string;
  childThreadTitle?: string;
  status: ChildCompletionStatus;
  summary: string;
  pendingChangeIds?: string[];
  subThreadPendingChanges?: SubThreadPendingChange[];
  heldNote?: string;
  filed?: boolean;
}

export const FILED_REPORT_LINE = 'Filed without waking this thread';

const CHILD_STATE: Record<ChildCompletionStatus, { label: string; tone: EventRowTone }> = {
  success: { label: 'Success', tone: 'good' },
  failure: { label: 'Failure', tone: 'bad' },
  no_changes: { label: 'No changes', tone: 'none' },
  canceled: { label: 'Canceled', tone: 'halted' },
  interrupted: { label: 'Interrupted', tone: 'halted' },
};

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

export function ChildCompletionRow(props: Props) {
  const { label, tone } = CHILD_STATE[props.status];
  const summaryHtml = props.summary.trim() ? renderMarkdown(props.summary) : '';
  const pending = props.pendingChangeIds?.length ?? 0;
  const below = props.subThreadPendingChanges?.length ?? 0;
  return eventRowBody({
    kind: 'child',
    state: props.status,
    role: 'child-completion',
    subject: eventNameChip({
      kind: 'chip',
      name: props.filed ? 'ChildReportFiled' : 'ChildThreadCompleted',
      sentenceStart: true,
    }),
    stateLabel: label,
    tone,
    heldNote: props.heldNote,
    facts: [
      { kind: 'node' as const, node: threadLink(props.childThreadId, props.childThreadTitle) },
      props.filed ? { kind: 'text' as const, text: FILED_REPORT_LINE } : null,
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
            <div class="markdown-content authored-html" dangerouslySetInnerHTML={{ __html: summaryHtml }} />
          ),
        }
      : undefined,
  });
}

interface StoppedProps {
  childThreadId: string;
  childThreadTitle?: string;
}

function childStoppedState(childThreadId: string): { label: string; tone: EventRowTone } {
  const child = threadMap.value.get(childThreadId);
  if (!child) return { label: 'Stopped', tone: 'halted' };
  if (child.meta.isStoppedChild) return { label: 'Waiting for you', tone: 'live' };
  if (isMidTurn(effectiveThreadStatus(child))) return { label: 'Continued', tone: 'live' };
  return { label: 'No longer waiting', tone: 'none' };
}

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
