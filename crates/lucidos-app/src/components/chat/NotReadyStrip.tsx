import { useRef } from 'preact/hooks';
import { hardenThread } from '../../api/client';
import type { SuccessorLink } from '../../api/threads';
import type { UnproposedReason } from '../../generated/thread-event-wire';
import type { ThreadMeta } from '../../store/thread-events';
import { focusedThreadId, notReadyReason, showToast, threadMap } from '../../store/store';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';
import { continueStoppedThread } from './continueStoppedThread';
import { successorLinkTitle } from './SuccessorNotice';

/** What the strip says for each reason a turn end withheld the work (ADR 0400).
 *  `hint` sits on the strip, and a tap toasts `detail`. */
export const NOT_READY_COPY: Record<StripReason, { label: string; hint: string; detail: string }> = {
  plan_missing: {
    label: 'Not ready: needs a plan',
    hint: 'Ask the agent for an implementation plan, then approve it.',
    detail:
      'Lucidos held this work back because its branch has no implementation plan. Ask the agent for a plan and approve it. Its next turn end then proposes the change.',
  },
  plan_awaiting_approval: {
    label: 'Not ready: plan awaits approval',
    hint: 'Approve the plan, and the agent finishes the work.',
    detail:
      'Lucidos held this work back because its implementation plan is not approved yet. Approve the plan, and the agent proposes the change once it finishes.',
  },
  outside_bound: {
    label: 'Not ready: outside its fix bound',
    hint: 'The fix changes files its security marker did not name.',
    detail:
      'Lucidos held this work back because the security fix reaches past the files it named. The agent can drop the extra files, name them all, or write a plan for you to approve.',
  },
  hardening_missing: {
    label: 'Not ready: not hardened',
    hint: 'Harden runs the checks, then the work moves on.',
    detail:
      'Lucidos held this work back because it has not been hardened, so Apply would have to harden it first. Harden reviews the work and runs its tests. When it finishes, Lucidos proposes the change, or an integrated sub-thread hands it to its parent.',
  },
  turn_incomplete: {
    label: 'Not ready: the turn did not finish',
    hint: 'Continue resumes it.',
    detail:
      'The turn that made this work was stopped or cut short, so Lucidos did not propose it. Continue resumes the turn, and a turn that finishes proposes the change.',
  },
  awaiting_landing: {
    label: 'Not ready: waiting for the landing lane',
    hint: 'It is proposed once the queued change lands.',
    detail:
      'This thread has a change waiting in the landing lane, which lands exactly the commit Apply queued. Lucidos proposes the newer work once that change lands, or its batch fails.',
  },
  integrated_into_parent: {
    label: 'Integrated into its parent',
    hint: "Its parent's agent merges this branch.",
    detail:
      "This is an integrated sub-thread. Its work folds into its parent's branch, so Lucidos never proposes it here. The parent's agent merges this branch, and the parent's change carries the work to Apply.",
  },
};

/** Every reason but a hand-off: that one is no warning, so it draws as a
 *  quiet link to the successor thread instead of the amber strip. */
type StripReason = Exclude<UnproposedReason, 'carried_by_successor'>;

/** The strip's label for `reason`. An integrated sub-thread names the parent
 *  it folds into, once the title is known. */
export function notReadyLabel(reason: StripReason, meta: Pick<ThreadMeta, 'parentThreadTitle'> | undefined): string {
  if (reason === 'integrated_into_parent' && meta?.parentThreadTitle) return `Integrated into ${meta.parentThreadTitle}`;
  return NOT_READY_COPY[reason].label;
}

type Shown =
  | { threadId: string; reason: StripReason; label: string }
  | { threadId: string; reason: 'carried_by_successor'; successor: SuccessorLink | null };

function shownFor(threadId: string, reason: UnproposedReason, meta: ThreadMeta | undefined): Shown {
  if (reason === 'carried_by_successor') return { threadId, reason, successor: meta?.successor ?? null };
  return { threadId, reason, label: notReadyLabel(reason, meta) };
}

/** What sits above the prompt box when the focused thread's work is not
 *  ready, where Apply would otherwise be offered: the amber strip saying why,
 *  or a link to the successor thread that carries the work on. */
export function NotReadyStrip() {
  const threadId = focusedThreadId.value;
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const reason = notReadyReason(thread);
  // The last reason shown, so the strip keeps its words while it rolls shut.
  const shown = useRef<Shown | null>(null);
  if (threadId && reason) shown.current = shownFor(threadId, reason, thread?.meta);
  const last = shown.current;
  return (
    <Disclosure open={reason !== null}>
      {last?.reason === 'carried_by_successor' ? (
        <ContinuedHereLink successor={last.successor} />
      ) : last && (
        <div class="not-ready-strip" data-reason={last.reason}>
          <button
            class="not-ready-strip-reason"
            onClick={() => showToast(NOT_READY_COPY[last.reason].detail, 'info', { title: last.label })}
          >
            <WarningIcon />
            <span class="not-ready-strip-text">
              <span class="not-ready-strip-label">{last.label}</span>
              <span class="not-ready-strip-hint">{` · ${NOT_READY_COPY[last.reason].hint}`}</span>
            </span>
          </button>
          {last.reason === 'turn_incomplete' && (
            <button class="action-btn protected-surface" onClick={() => void continueStoppedThread(last.threadId)}>
              Continue
            </button>
          )}
          {last.reason === 'hardening_missing' && (
            <button class="action-btn protected-surface" onClick={() => void startHardening(last.threadId)}>
              Harden
            </button>
          )}
        </div>
      )}
    </Disclosure>
  );
}

/** "Continued here", where "here" opens the successor thread. Before its link
 *  loads there is nowhere to go, so it says so without one. */
function ContinuedHereLink({ successor }: { successor: SuccessorLink | null }) {
  return (
    <p class="continued-here" data-role="continued-here">
      <ArrowIcon />
      <span>
        {successor ? (
          <>
            {'Continued '}
            <button
              type="button"
              class="continued-here-link"
              data-tooltip={successorLinkTitle(successor) ?? undefined}
              onClick={() => focusThreadOrBootstrap(successor.thread_id)}
            >
              here
            </button>
          </>
        ) : (
          'Continued in a new thread'
        )}
      </span>
    </p>
  );
}

/** Start the hardening run the strip offers. A failure toasts, since the
 *  strip stays up and the press can be retried. */
async function startHardening(threadId: string): Promise<void> {
  try {
    await hardenThread(threadId);
  } catch (err) {
    showToast(`Failed to start hardening: ${errorDetail(err)}`, 'error');
  }
}

function ArrowIcon() {
  return (
    <svg class="continued-here-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <line x1="5" y1="12" x2="19" y2="12" />
      <polyline points="12 5 19 12 12 19" />
    </svg>
  );
}

function WarningIcon() {
  return (
    <svg class="not-ready-strip-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
      <line x1="12" y1="9" x2="12" y2="13" />
      <line x1="12" y1="17" x2="12.01" y2="17" />
    </svg>
  );
}
