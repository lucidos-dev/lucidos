import type { CodingAgentCommandOption, CodingAgentLadderRung } from '../../api/client';
import type { ControlOption } from '../shared/ControlOptionList';
import { LADDER_LINK_VALUE, isRung } from './rungRows';

export interface RungPosition {
  /** The rung's place on the ladder, 1 at the top. */
  at: number;
  /** How many rungs the ladder lists. */
  of: number;
}

export interface ShownRung {
  /** A rung's label, an off-ladder model's own name, or the backend's name
   *  when its own config picks the model. */
  name: string;
  effort: string | null;
  /** Where the pair sits on the ladder, or null when the ladder lacks it. */
  position: RungPosition | null;
  /** The rung's intelligence band (`1.12×`), the badge a narrow pill shows.
   *  Null off the ladder or with no estimate, and the pill keeps its name. */
  intelligence: string | null;
}

export interface ShownRungInput {
  /** False until a commands read has answered, which leaves nothing to show. */
  loaded: boolean;
  rungs: readonly CodingAgentLadderRung[];
  options: readonly CodingAgentCommandOption[];
  /** The pair in force. A null or `default` model is nothing picked. */
  model: string | null;
  effort: string | null;
  /** What nothing-picked runs as: the backend's default implementer, when set. */
  defaultPair: { model: string; effort: string | null } | null;
  /** The backend, which picks the model itself when no default is set. */
  agentName: string;
  modelLabel: (model: string) => string;
}

/** What the pill names for a model that offers tiers and has none recorded. */
export const UNSET_EFFORT_LABEL = 'default';

/** What the control names: the pair in force, else the default pair, else the
 *  backend. Never a bare "Default", which names no model. A model that offers
 *  tiers never shows alone: a missing effort reads as the model's default. */
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

/** The band label a rung's badge shows, or null when it has no estimate. */
export function rungIntelligence(rung: Pick<CodingAgentLadderRung, 'intelligence'>): string | null {
  return rung.intelligence?.label ?? null;
}

/** The rows one step up or down the rung list from the row in force. A step
 *  never wraps, so the first and last rows have no target in one direction.
 *  The ladder link is a way out of the list, never a rung to land on. */
export function stepTargets(
  rows: readonly ControlOption[],
  current: string,
): { prev: string | null; next: string | null } {
  const rungs = rows.filter((r) => r.value !== LADDER_LINK_VALUE);
  const at = rungs.findIndex((r) => r.value === current);
  if (at === -1) return { prev: null, next: null };
  return { prev: rungs[at - 1]?.value ?? null, next: rungs[at + 1]?.value ?? null };
}
