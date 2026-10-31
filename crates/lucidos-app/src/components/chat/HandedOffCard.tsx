import { useEffect, useRef, useState } from 'preact/hooks';
import { focusedThreadId, threadMap } from '../../store/store';
import { draftPresentThreadIds } from '../../store/composeDrafts';
import { latestSuccessor, moveDraftToSuccessor } from '../../store/actions/successor';
import { Disclosure } from '../shared/Disclosure';
import { threadLink } from './ChildCompletionRow';
import { successorLinkTitle } from './SuccessorNotice';
import { CardOption } from './SuccessorCard';

/** A warning on top of the composer of a thread that handed off to a
 *  *legacy successor thread*, once the user starts writing there. Commits made here
 *  are set aside once the successor's change lands (ADR 0474). *Send here
 *  anyway* hides it until the composer is cleared. */
export function HandedOffCard() {
  const threadId = focusedThreadId.value;
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const writing = threadId !== null && draftPresentThreadIds.value.has(threadId);
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);

  useEffect(() => {
    if (!writing && dismissedFor === threadId) setDismissedFor(null);
  }, [writing, threadId, dismissedFor]);

  const target = threadId && thread?.meta.successor ? latestSuccessor(threadId) : null;
  const open = target !== null && writing && dismissedFor !== threadId;

  // The last card shown, so it keeps its words while it rolls shut.
  const shown = useRef<{ threadId: string; target: string } | null>(null);
  if (open && threadId && target) shown.current = { threadId, target };
  const last = shown.current;
  const direct = thread?.meta.successor;
  const title = !last ? null
    : direct?.thread_id === last.target ? successorLinkTitle(direct)
    : threadMap.value.get(last.target)?.meta.title ?? null;

  return (
    <Disclosure open={open}>
      {last && (
        <div class="question-body successor-card protected-surface" data-role="handed-off-card">
          <div class="question-text">
            <p>
              {'This thread already continued in '}{threadLink(last.target, title)}
              {'. Commits made here are set aside once that change lands.'}
            </p>
          </div>
          <div class="question-options">
            <CardOption
              label="Move my message there"
              desc="Opens that thread with what you typed as its draft, unsent, unless it already holds one."
              busyDesc=""
              busy={false}
              disabled={false}
              onPick={() => moveDraftToSuccessor(last.threadId)}
            />
            <CardOption
              label="Send here anyway"
              desc="Hides this warning until the composer is cleared."
              busyDesc=""
              busy={false}
              disabled={false}
              onPick={() => setDismissedFor(last.threadId)}
            />
          </div>
        </div>
      )}
    </Disclosure>
  );
}
