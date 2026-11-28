import type { CodingAgentCommandOption, CodingAgentLadderRung } from '../../api/client';
import type { ControlOption } from '../shared/ControlOptionList';
import { LADDER_LINK_VALUE, isRung } from './rungRows';

export interface RungPosition {
  at: number;
  of: number;
}

export interface ShownRung {
  nextTurn?: string;
  name: string;
  effort: string | null;
  position: RungPosition | null;
  intelligence: string | null;
}

export interface ShownRungInput {
  loaded: boolean;
  rungs: readonly CodingAgentLadderRung[];
  options: readonly CodingAgentCommandOption[];
  model: string | null;
  effort: string | null;
  defaultPair: { model: string; effort: string | null } | null;
  agentName: string;
  modelLabel: (model: string) => string;
}

export const UNSET_EFFORT_LABEL = 'default';

export function shownRung(input: ShownRungInput): ShownRung | null {
  if (!input.loaded) return null;
  const picked = input.model !== null && input.model !== 'default';
  const pair = picked ? { model: input.model as string, effort: input.effort } : input.defaultPair;
  if (!pair) return { name: input.agentName, effort: null, position: null, intelligence: null };
  const at = input.rungs.findIndex((r) => isRung(input.options, r, pair.model, pair.effort));
  const offersTiers = (input.options.find((o) => o.value === pair.model)?.reasoning_efforts ?? []).length > 0;
  return {
    name: at === -1 ? input.modelLabel(pair.model) : input.rungs[at].label,
    effort: pair.effort ?? (offersTiers ? UNSET_EFFORT_LABEL : null),
    position: at === -1 ? null : { at: at + 1, of: input.rungs.length },
    intelligence: at === -1 ? null : rungIntelligence(input.rungs[at]),
  };
}

export function rungIntelligence(rung: Pick<CodingAgentLadderRung, 'intelligence'>): string | null {
  return rung.intelligence?.label ?? null;
}

export function stepTargets(
  rows: readonly ControlOption[],
  current: string,
): { prev: string | null; next: string | null } {
  const rungs = rows.filter((r) => r.value !== LADDER_LINK_VALUE);
  const at = rungs.findIndex((r) => r.value === current);
  if (at === -1) return { prev: null, next: null };
  return { prev: rungs[at - 1]?.value ?? null, next: rungs[at + 1]?.value ?? null };
}
