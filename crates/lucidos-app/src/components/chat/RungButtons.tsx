import type { CodingAgent } from '../../api/types';
import { ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { keepFocusOnPress } from '../../utils/dom';
import type { ShownRung } from './rungControlParts';

export interface RungControlProps {
  /** The thread the control belongs to: a coding agent's active session, or
   *  the focused Lucidos Agent thread (a draft's id in compose). */
  threadId?: string;
  composeThreadId?: string;
  /** Lucidos Agent threads only: true for a composing draft and the fresh
   *  compose view. */
  composeContext?: boolean;
  /** The coding-agent backend, or null for a Lucidos Agent thread. */
  codingAgent?: CodingAgent | null;
  /** True once the composer row has folded the pill into the ⋯ menu: the
   *  control then draws no box and opens its list against the ⋯. */
  folded?: boolean;
  /** The row's attributes for the pill, so the composer row can measure and
   *  fold it like any other member. */
  rowAttrs?: Record<string, string>;
}

/** The pill's name in the row's fold, which remembers its width. */
export const RUNG_FOLD_KEY = 'rung';

/** How far the pill shortens before the row folds anything: full, then badge
 *  and name, then the badge alone (`styles/chat/rung-control.css`). */
export const RUNG_SHED_LEVELS = 3;

/** A ladder glyph for the folded pill's ⋯ menu row. */
export function RungMenuIcon() {
  return (
    <svg class="rung-menu-icon" viewBox="0 0 20 16" aria-hidden="true">
      {[0, 1, 2, 3, 4].map((i) => (
        <rect key={i} x={i * 4} y={12 - i * 2.4} width="3" height={i * 2.4 + 4} rx="1" />
      ))}
    </svg>
  );
}

/** The band in force, stacked over every other band the pill can show, so
 *  every badge is as wide as the widest. */
function RungIntelligence({ band, bands }: { band: string; bands: readonly string[] }) {
  return (
    <span class="rung-intelligence">
      <span>{band}</span>
      {bands.filter((b) => b !== band).map((b) => (
        <span key={b} class="rung-intelligence-reserve" aria-hidden="true">{b}</span>
      ))}
    </span>
  );
}

/** `1.12× Opus 5.5 · medium`. A narrow bar sheds the effort and then the
 *  name, and only a narrow bar shows the intelligence badge. */
function RungFace({ shown, bands }: { shown: ShownRung; bands: readonly string[] }) {
  return (
    <>
      {shown.intelligence && <RungIntelligence band={shown.intelligence} bands={bands} />}
      <span class="rung-name">{shown.name}</span>
      {shown.effort && <span class="rung-effort">{` · ${shown.effort}`}</span>}
    </>
  );
}

/** The face in force, stacked over every other face the pill can show. Only
 *  the one in force is visible, and the widest sets the width, so a step
 *  never resizes the pill. */
function RungLabel({ shown, faces }: { shown: ShownRung; faces: readonly ShownRung[] }) {
  const bands = [...new Set(faces.flatMap((f) => f.intelligence ?? []))];
  return (
    <span class="rung-label">
      <span class="rung-face"><RungFace shown={shown} bands={bands} /></span>
      {faces.map((face, i) => (
        <span key={i} class="rung-face rung-face-reserve" aria-hidden="true"><RungFace shown={face} bands={bands} /></span>
      ))}
    </span>
  );
}

/** The accessible name for the part that opens the list. */
function openLabel(shown: ShownRung | null): string {
  if (!shown) return 'Choose model and effort';
  const rung = shown.position ? `, rung ${shown.position.at} of ${shown.position.of}` : '';
  return shown.effort
    ? `Model and effort: ${shown.name}, ${shown.effort}${rung}. Choose another`
    : `Model: ${shown.name}${rung}. Choose another`;
}

/** A pill the host gives no row attributes is still measured, never folded. */
const PINNED_ROW_ATTRS = { 'data-row-item': 'true' };

type RungSteps = { prev: (() => void) | null; next: (() => void) | null };

/** ← and → on any part of the pill step the rung, as ‹ and › do. No
 *  arrow on a button belongs to text editing, so none is taken. */
function stepOnArrow(step: RungSteps | undefined, e: KeyboardEvent): void {
  if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
  e.preventDefault();
  (e.key === 'ArrowLeft' ? step?.prev : step?.next)?.();
}

/** One end of the pill: ‹ steps to the previous rung, › to the next. */
function RungStep({ dir, step, lockedReason }: {
  dir: 'prev' | 'next';
  step: RungSteps | undefined;
  lockedReason: string | null;
}) {
  const label = dir === 'prev' ? 'Previous model and effort' : 'Next model and effort';
  return (
    <button
      type="button"
      class="rung-step"
      aria-label={label}
      data-tooltip={lockedReason ?? label}
      aria-disabled={lockedReason !== null || !step?.[dir]}
      onMouseDown={keepFocusOnPress}
      onKeyDown={(e) => stepOnArrow(step, e)}
      onClick={() => step?.[dir]?.()}
    >
      {dir === 'prev' ? <ChevronLeftIcon size="0.75rem" /> : <ChevronRightIcon size="0.75rem" />}
    </button>
  );
}

/** The control's boxes, with no data of its own. The real control and the
 *  placeholder shown until its code arrives both draw them, so the box never
 *  changes as it loads. */
export function RungButtons({
  shown, faces = [], rowAttrs = PINNED_ROW_ATTRS, expanded, triggerRef, onOpen, step, lockedReason = null,
}: {
  rowAttrs?: Record<string, string>;
  /** What the control names, or null until its face is whole. A null face
   *  draws the boxes unseen, so the pill appears once, at its final width. */
  shown: ShownRung | null;
  /** Every other face the pill can show, which it reserves room for. */
  faces?: readonly ShownRung[];
  expanded: boolean;
  triggerRef?: (el: HTMLButtonElement | null) => void;
  onOpen: (e: MouseEvent) => void;
  /** The pill's two steps through the rung list. A null step is one
   *  the list has no row for, which leaves its button disabled. Absent until
   *  the commands have loaded. */
  step?: RungSteps;
  /** Why the rung cannot change now, which dims the pill and makes it inert.
   *  A press still reaches `onOpen` and the steps, which say why. */
  lockedReason?: string | null;
}) {
  return (
    <div
      class="rung-control rung-pill"
      data-locked={lockedReason !== null || undefined}
      data-pending={shown === null || undefined}
      {...rowAttrs}
    >
      <RungStep dir="prev" step={step} lockedReason={lockedReason} />
      <button
        type="button"
        class="rung-trigger"
        data-tooltip={lockedReason ?? tooltipWithShortcut('Model and effort', 'openAgentMenu')}
        aria-disabled={lockedReason !== null || undefined}
        ref={triggerRef}
        aria-label={openLabel(shown)}
        aria-haspopup="listbox"
        aria-expanded={expanded}
        onMouseDown={keepFocusOnPress}
        onKeyDown={(e) => stepOnArrow(step, e)}
        onClick={onOpen}
      >
        {shown && <RungLabel shown={shown} faces={faces} />}
        <ChevronDownIcon size="0.75rem" />
      </button>
      <RungStep dir="next" step={step} lockedReason={lockedReason} />
    </div>
  );
}
