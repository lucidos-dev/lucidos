import type { CodingAgent } from '../../api/types';
import { ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { keepFocusOnPress } from '../../utils/dom';
import type { ShownRung } from './rungControlParts';

export interface RungControlProps {
  threadId?: string;
  composeThreadId?: string;
  disabledReason?: string | null;
  composeContext?: boolean;
  codingAgent?: CodingAgent | null;
  folded?: boolean;
  rowAttrs?: Record<string, string>;
}

export const RUNG_FOLD_KEY = 'rung';

export function RungMenuIcon() {
  return (
    <svg class="rung-menu-icon" viewBox="0 0 20 16" aria-hidden="true">
      {[0, 1, 2, 3, 4].map((i) => (
        <rect key={i} x={i * 4} y={12 - i * 2.4} width="3" height={i * 2.4 + 4} rx="1" />
      ))}
    </svg>
  );
}

function RungLabel({ shown }: { shown: ShownRung }) {
  return (
    <span class="rung-label">
      {shown.intelligence && <span class="rung-intelligence">{shown.intelligence}</span>}
      <span class="rung-name">{shown.name}</span>
      {shown.effort && <span class="rung-effort">{` · ${shown.effort}`}</span>}
      {shown.nextTurn && <span class="rung-next-turn" aria-hidden="true">*</span>}
    </span>
  );
}

function pairName(shown: ShownRung | null): string {
  if (!shown) return 'Model and effort';
  if (shown.nextTurn) return `Running: ${shown.name}${shown.effort ? ` · ${shown.effort}` : ''}. Next turn: ${shown.nextTurn}`;
  return shown.effort ? `${shown.name} · ${shown.effort}` : shown.name;
}

function openLabel(shown: ShownRung | null): string {
  if (!shown) return 'Choose model and effort';
  if (shown.nextTurn) return `${pairName(shown)}. Choose another`;
  const rung = shown.position ? `, rung ${shown.position.at} of ${shown.position.of}` : '';
  return shown.effort
    ? `Model and effort: ${shown.name}, ${shown.effort}${rung}. Choose another`
    : `Model: ${shown.name}${rung}. Choose another`;
}

const PINNED_ROW_ATTRS = { 'data-row-item': 'true' };

type RungSteps = { prev: (() => void) | null; next: (() => void) | null };

function stepOnArrow(step: RungSteps | undefined, e: KeyboardEvent): void {
  if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
  e.preventDefault();
  (e.key === 'ArrowLeft' ? step?.prev : step?.next)?.();
}

function RungStep({ dir, step, disabledReason, lockedReason }: {
  dir: 'prev' | 'next';
  disabledReason?: string | null;
  lockedReason?: string | null;
  step: RungSteps | undefined;
}) {
  const label = dir === 'prev' ? 'Previous model and effort' : 'Next model and effort';
  return (
    <button
      type="button"
      class="rung-step"
      aria-label={label}
      data-tooltip={disabledReason ?? lockedReason ?? label}
      aria-disabled={!!disabledReason || !step?.[dir]}
      onMouseDown={keepFocusOnPress}
      onKeyDown={(e) => stepOnArrow(disabledReason ? undefined : step, e)}
      onClick={() => { if (!disabledReason) step?.[dir]?.(); }}
    >
      {dir === 'prev' ? <ChevronLeftIcon size="0.75rem" /> : <ChevronRightIcon size="0.75rem" />}
    </button>
  );
}

export function RungButtons({
  shown, rowAttrs = PINNED_ROW_ATTRS, expanded, triggerRef, onOpen, step, disabledReason, lockedReason,
}: {
  rowAttrs?: Record<string, string>;
  disabledReason?: string | null;
  lockedReason?: string | null;
  shown: ShownRung | null;
  expanded: boolean;
  triggerRef?: (el: HTMLButtonElement | null) => void;
  onOpen: (e: MouseEvent) => void;
  step?: RungSteps;
}) {
  return (
    <div
      class="rung-control rung-pill"
      data-pending={shown === null || undefined}
      {...rowAttrs}
    >
      <RungStep dir="prev" step={step} disabledReason={disabledReason} lockedReason={lockedReason} />
      <button
        type="button"
        class="rung-trigger"
        data-tooltip={disabledReason ?? lockedReason ?? tooltipWithShortcut(pairName(shown), 'openAgentMenu')}
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
      <RungStep dir="next" step={step} disabledReason={disabledReason} lockedReason={lockedReason} />
    </div>
  );
}
