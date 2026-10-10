import { getDraft } from '../../store/composeDrafts';
import { focusedThreadId } from '../../store/store';
import { postClientLog } from '../../utils/clientLog';
import { isMobile } from '../../utils/viewport';
import { activePromptInput } from './promptFocus';
import { readViewport } from './probeViewport';

/** Reports a keystroke that went into the composer and did not come out.
 *
 *  The sibling of `deadPressProbe`, and registered beside it in
 *  `docs/temporary-measures.md` § 1. That one watches the action row's buttons.
 *  This one watches the textarea, which nine reports of a dead composer went
 *  through without leaving a single line.
 *
 *  The ninth report is what shaped it: the box took focus, the keyboard came
 *  up, the user typed, and the characters never appeared. The press probe was
 *  silent and correct, no button having been pressed. The plan is
 *  `docs/plans/2026-08-29-the-composer-never-erases-what-you-typed.md`. */

/** ONE channel, the engine log, and no toast. A toast reports only to whoever
 *  is looking at the screen and keeps it, which is how five press episodes
 *  produced nothing to work from. Two of the three verdicts below also describe
 *  a fault the composer already repaired, so there is nothing for the user to
 *  do about them.
 *
 *  It carries no draft text and no message content: what the user typed has no
 *  business in a log line (`.claude/rules/no-private-data.md`). Lengths and
 *  flags only. */

/** How a keystroke goes missing. Three, and they partition the path a character
 *  takes from the keyboard to the screen.
 *
 *  `input-never-arrived` is the one that says the fault is not ours: WebKit
 *  announced an edit and then delivered none. `keystroke-lost` is the edit
 *  reaching the box and not the store. `draft-clobbered` is both of those
 *  working and a clear wiping the result. */
export type KeystrokeVerdict = 'draft-clobbered' | 'keystroke-lost' | 'input-never-arrived';

/** How long an announced edit has to actually arrive. Well past a frame, and
 *  short enough that the line still belongs to the keystroke that caused it. */
const INPUT_DEADLINE_MS = 400;

/** How long after an `input` the draft is given to catch up. The composer writes
 *  it synchronously in its `onInput`, so one task is already generous. */
const DRAFT_SETTLE_MS = 0;

/** Lines one wedged state may write before it goes quiet. A wedge persists, and
 *  the user's answer to a dead box is to keep typing, so an ungated probe would
 *  write a line per character. The count resets the moment a keystroke lands
 *  cleanly, so a state that returns reports again. */
const EPISODE_LINE_CAP = 5;

let installed = false;
let linesThisEpisode = 0;

function record(verdict: KeystrokeVerdict, data: Record<string, unknown>): void {
  if (linesThisEpisode >= EPISODE_LINE_CAP) return;
  linesThisEpisode += 1;
  postClientLog('composer-typing', verdict, {
    ...data,
    verdict,
    capped: linesThisEpisode >= EPISODE_LINE_CAP,
    viewport: readViewport(),
  });
}

/** A keystroke reached the draft, so whatever was wrong is over. */
function noteHealthy(): void {
  linesThisEpisode = 0;
}

/** The composer repaired an empty draft that reached a box holding typed
 *  characters. Called from `PromptInput`'s sync effect, the only place that can
 *  see it: `resolveEmptyDraftSync` answered `adopt`.
 *
 *  Not a toast, deliberately. The characters are still on screen and the draft
 *  now holds them, so the user has nothing to do and nothing to read. The line
 *  records that a clear ran under their fingers, which is the fault itself. */
export function reportDraftClobbered(facts: {
  charCount: number;
  threadStatus: string;
}): void {
  record('draft-clobbered', facts);
}

/** Install the probe. Idempotent, and gated per verdict rather than as a whole.
 *
 *  `input-never-arrived` is mobile-only: it asks whether WebKit delivered the
 *  edit it announced, which is an iOS PWA question, and a desktop typing path
 *  has never been in doubt. The other two are platform-independent, so they run
 *  everywhere.
 *
 *  Every listener is passive and calls neither `preventDefault` nor
 *  `stopPropagation`. A diagnostic that consumes a keystroke becomes the bug. */
export function installDeadKeystrokeProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;

  /** The announced edit waiting for its `input`, and the deadline that rules on
   *  it. One slot: a burst of edits is one episode, and the cap covers repeats. */
  let announced: { inputType: string; timer: ReturnType<typeof setTimeout> } | null = null;

  const clearAnnounced = () => {
    if (!announced) return;
    clearTimeout(announced.timer);
    announced = null;
  };

  document.addEventListener('beforeinput', (e) => {
    if (!isMobile()) return;
    const el = e.target as HTMLElement | null;
    if (el?.dataset?.role !== 'prompt-input') return;
    clearAnnounced();
    const inputType = (e as InputEvent).inputType || 'unknown';
    announced = {
      inputType,
      timer: setTimeout(() => {
        announced = null;
        // Only while the box still holds focus. A blur, a thread switch or a
        // send between the two events legitimately drops the edit, and none of
        // those is the pipeline stopping.
        if (!activePromptInput()) return;
        record('input-never-arrived', { inputType });
      }, INPUT_DEADLINE_MS),
    };
  }, { capture: true, passive: true });

  document.addEventListener('input', (e) => {
    const el = e.target as HTMLTextAreaElement | null;
    if (el?.dataset?.role !== 'prompt-input') return;
    clearAnnounced();
    setTimeout(() => {
      // Re-read the box, never the value captured at input time. The composer
      // legitimately rewrites it inside its own `onInput`: a leading "/" opens
      // the slash menu and empties the field, and a send clears it. Comparing
      // the captured value would report each of those as a lost keystroke.
      const domText = el.value;
      // Whitespace alone is not content, and on a composing thread the composer
      // auto-discards a draft the user emptied. Comparing it would report the
      // app's own correct behaviour the same way.
      if (domText.trim().length === 0) { noteHealthy(); return; }
      const threadId = focusedThreadId.value;
      const draft = getDraft(threadId).text;
      if (draft === domText) { noteHealthy(); return; }
      // The edit reached the box and the store does not have it. The composer's
      // own `onInput` writes the draft synchronously, so one task later this is
      // a write that did not happen, or landed elsewhere.
      record('keystroke-lost', {
        charCount: domText.length,
        draftCharCount: draft.length,
        hasFocusedThread: threadId !== null,
      });
    }, DRAFT_SETTLE_MS);
  }, { capture: true, passive: true });
}

/** Test-only reset. Vitest mounts a fresh document per file, and the
 *  install-once latch plus the episode counter have to follow. */
export function _resetDeadKeystrokeProbeForTesting(): void {
  installed = false;
  linesThisEpisode = 0;
}
