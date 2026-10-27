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
  disabledReason?: string | null;
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

/** `1.12× Opus 5.5 · medium`. The desktop face shows the name and effort,
 *  the compact face the intelligence badge alone (`styles/chat/rung-control.css`). */
function RungLabel({ shown }: { shown: ShownRung }) {
  return (
    <span class="rung-label">
      {shown.intelligence && <span class="rung-intelligence">{shown.intelligence}</span>}
      <span class="rung-name">{shown.name}</span>
      {shown.effort && <span class="rung-effort">{` · ${shown.effort}`}</span>}
    </span>
  );
}

/** The pair in full, which the label may cut short with an ellipsis. */
function pairName(shown: ShownRung | null): string {
  if (!shown) return 'Model and effort';
  return shown.effort ? `${shown.name} · ${shown.effort}` : shown.name;
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
function RungStep({ dir, step, disabledReason }: {
  dir: 'prev' | 'next';
  disabledReason?: string | null;
  step: RungSteps | undefined;
}) {
  const label = dir === 'prev' ? 'Previous model and effort' : 'Next model and effort';
  return (
    <button
      type="button"
      class="rung-step"
      aria-label={label}
      data-tooltip={disabledReason ?? label}
      aria-disabled={!!disabledReason || !step?.[dir]}
      onMouseDown={keepFocusOnPress}
      onKeyDown={(e) => stepOnArrow(disabledReason ? undefined : step, e)}
      onClick={() => { if (!disabledReason) step?.[dir]?.(); }}
    >
      {dir === 'prev' ? <ChevronLeftIcon size="0.75rem" /> : <ChevronRightIcon size="0.75rem" />}
    </button>
  );
}

/** The control's boxes, with no data of its own. The real control and the
 *  placeholder shown until its code arrives both draw them, so the box never
 *  changes as it loads. */
export function RungButtons({
  shown, rowAttrs = PINNED_ROW_ATTRS, expanded, triggerRef, onOpen, step, disabledReason,
}: {
  rowAttrs?: Record<string, string>;
  disabledReason?: string | null;
  /** What the control names, or null until its face is whole. A null face
   *  keeps a visible label until the selection arrives. */
  shown: ShownRung | null;
  expanded: boolean;
  triggerRef?: (el: HTMLButtonElement | null) => void;
  onOpen: (e: MouseEvent) => void;
  /** The pill's two steps through the rung list. A null step is one
   *  the list has no row for, which leaves its button disabled. Absent until
   *  the commands have loaded. */
  step?: RungSteps;
}) {
  return (
    <div
      class="rung-control rung-pill"
      data-pending={shown === null || undefined}
      {...rowAttrs}
    >
      <RungStep dir="prev" step={step} disabledReason={disabledReason} />
      <button
        type="button"
        class="rung-trigger"
        data-tooltip={disabledReason ?? tooltipWithShortcut(pairName(shown), 'openAgentMenu')}
        aria-disabled={!!disabledReason}
        ref={triggerRef}
        aria-label={openLabel(shown)}
        aria-haspopup="listbox"
        aria-expanded={expanded}
        onMouseDown={keepFocusOnPress}
        onKeyDown={(e) => stepOnArrow(disabledReason ? undefined : step, e)}
        onClick={(e) => { if (!disabledReason) onOpen(e); }}
      >
        {shown ? <RungLabel shown={shown} /> : <span class="rung-label">Model and effort</span>}
        <ChevronDownIcon size="0.75rem" />
      </button>
      <RungStep dir="next" step={step} disabledReason={disabledReason} />
    </div>
  );
}
