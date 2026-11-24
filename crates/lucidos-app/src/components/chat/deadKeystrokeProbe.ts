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


export type KeystrokeVerdict = 'keystroke-lost' | 'input-never-arrived';

const INPUT_DEADLINE_MS = 400;

const DRAFT_SETTLE_MS = 0;

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

function noteHealthy(): void {
  linesThisEpisode = 0;
}

export function installDeadKeystrokeProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;

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
      const domText = el.value;
      if (domText.trim().length === 0) { noteHealthy(); return; }
      const threadId = focusedThreadId.value;
      const draft = getDraft(threadId).text;
      if (draft === domText) { noteHealthy(); return; }
      record('keystroke-lost', {
        charCount: domText.length,
        draftCharCount: draft.length,
        hasFocusedThread: threadId !== null,
      });
    }, DRAFT_SETTLE_MS);
  }, { capture: true, passive: true });
}

export function _resetDeadKeystrokeProbeForTesting(): void {
  installed = false;
  linesThisEpisode = 0;
}
