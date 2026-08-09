import type { CodingAgent } from '../../api/types';
import { CodingAgentControlMenu } from './CodingAgentControlMenu';
import { EventWaitIndicator } from './EventWaitPanel';
import { LucidosControlMenu } from './LucidosControlMenu';
import { TodoListIndicator } from './TodoListPanel';

/** The leading control cluster of `.prompt-actions-row`: whichever backend's
 *  control menu the focused thread resolves to, plus the indicators that belong
 *  beside it.
 *
 *  A Fragment, deliberately: `.prompt-actions-row` is a flex row and its
 *  children are diffed positionally (see `prompt-vdom-keys.test.ts`), so this
 *  groups the cluster for reading without putting a box between the row and its
 *  buttons.
 *
 *  **The split is by WHO OWNS the control, and only two of the three are
 *  backend-specific.** The control menu obviously is. `TodoListIndicator` is
 *  too, and it is the reason the branch exists at all: `TodoListWritten` is the
 *  Lucidos Agent's own todo list, so a coding-agent thread has none to show.
 *  The *subscription indicator* is NOT, and it sits outside the branch for the
 *  same reason `await_event` has a CLI mirror: an *event wait* belongs to the
 *  thread, and both agents arm them (`system-knowhow/glossary.md` §§ "Event
 *  wait", "Subscription indicator"; ADR 0052 item 3).
 *
 *  It used to live inside the Lucidos arm, and that hid it from exactly the
 *  threads that use it most. `.claude/skills/e2e-lock-wait/SKILL.md` exists to
 *  make a coding-agent thread subscribe rather than poll, so on 2026-08-09 one
 *  of them armed a six-hour watch on the e2e lock, sat in **Waiting** with the
 *  dot lit, and offered nowhere to read what it was watching and no **Stop
 *  waiting** button to end it. The dot reads the projected
 *  `liveEventWaitCount`, which is right on every thread; this is the surface
 *  that says what for, so it has to be mounted on every thread too. */
export function PromptRowControls({
  codingAgent,
  codingAgentThreadId,
  composeThreadId,
  lucidosThreadId,
  composeContext,
}: {
  /** The focused thread's resolved coding-agent backend, or `null` for a
   *  Lucidos Agent thread (`effectiveCodingAgentBackend`). */
  codingAgent: CodingAgent | null;
  /** The active coding-agent session's thread id (absent for a draft). */
  codingAgentThreadId: string | undefined;
  /** The composing draft's id (absent for an active thread), which keys the
   *  per-draft model / effort / scope picks. */
  composeThreadId: string | undefined;
  lucidosThreadId: string | undefined;
  composeContext: boolean;
}) {
  return (
    <>
      {codingAgent !== null ? (
        <CodingAgentControlMenu
          threadId={codingAgentThreadId}
          composeThreadId={composeThreadId}
          codingAgent={codingAgent}
        />
      ) : (
        <>
          <LucidosControlMenu threadId={lucidosThreadId} composeContext={composeContext} />
          <TodoListIndicator />
        </>
      )}
      <EventWaitIndicator />
    </>
  );
}
