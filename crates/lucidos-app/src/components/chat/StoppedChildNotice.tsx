import type { ThreadMeta } from '../../store/thread-events';
import { threadLink } from './ChildCompletionRow';
import { eventRowBody } from './EventRow';

/** The words the notice's two facts say, exported so a test can pin them
 *  without rendering. */
export const STOPPED_CHILD_CONTINUE = 'Send a message to continue it';
export const STOPPED_CHILD_SETTLE = 'or Archive it, or Discard its change, to tell the parent you are done';

/** What ends a *stopped child*'s transcript (ADR 0252): the reason it counts
 *  toward attention, said where the user lands.
 *
 *  A badge alone would point at a thread that ends on a bare stop line. So
 *  this names the parent that is waiting, and the two ways forward. It carries
 *  no buttons of its own: Archive and Discard already sit in the prompt row
 *  below it, and a second pair would be two controls for one act.
 *
 *  Renders nothing unless the thread is a stopped child, so the projection's
 *  flag alone decides it, and the next message clears it with the flag. */
export function StoppedChildNotice({ meta }: { meta: ThreadMeta }) {
  if (!meta.isStoppedChild || !meta.parentThreadId) return null;
  return (
    <div class="stopped-child-notice" data-role="stopped-child-notice">
      {eventRowBody({
        kind: 'child',
        mark: 'pending',
        state: 'stopped',
        role: 'stopped-child-notice-row',
        subject: (
          <>
            {'You stopped this thread, and its parent is waiting on it: '}
            {threadLink(meta.parentThreadId, meta.parentThreadTitle)}
          </>
        ),
        stateLabel: 'waiting for you',
        tone: 'halted',
        facts: [
          { kind: 'text', text: STOPPED_CHILD_CONTINUE },
          { kind: 'text', text: STOPPED_CHILD_SETTLE },
        ],
      })}
    </div>
  );
}
