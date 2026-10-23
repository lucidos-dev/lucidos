import type { SuccessorLink } from '../../api/threads';
import { threadMap } from '../../store/store';
import type { ThreadMeta } from '../../store/thread-events';
import { threadLink } from './ChildCompletionRow';
import { eventRowBody } from './EventRow';

/** The linked thread's title. The live row wins: the successor's row did not
 *  exist yet when the link landed, so the link's own title may be empty. */
export function successorLinkTitle(link: SuccessorLink): string | null {
  return threadMap.value.get(link.thread_id)?.meta.title ?? link.title;
}

/** What closes the feed of a thread that handed off: where its work went on.
 *  Renders nothing until the thread has a *successor thread*. */
export function ContinuedInNotice({ meta }: { meta: ThreadMeta }) {
  if (!meta.successor) return null;
  return (
    <div class="successor-notice" data-role="continued-in">
      {eventRowBody({
        kind: 'child',
        role: 'continued-in-row',
        subject: <>{'Continued in '}{threadLink(meta.successor.thread_id, successorLinkTitle(meta.successor))}</>,
        stateLabel: 'Handed off',
      })}
    </div>
  );
}

/** What opens a *successor thread*'s feed: the thread it continues. */
export function ContinuedFromNotice({ meta }: { meta: ThreadMeta }) {
  if (!meta.successorOf) return null;
  return (
    <div class="successor-notice" data-role="continued-from">
      {eventRowBody({
        kind: 'child',
        role: 'continued-from-row',
        subject: <>{'Continued from '}{threadLink(meta.successorOf.thread_id, successorLinkTitle(meta.successorOf))}</>,
      })}
    </div>
  );
}
