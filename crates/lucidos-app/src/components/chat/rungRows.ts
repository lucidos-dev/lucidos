import type { CodingAgentCommandOption, CodingAgentLadderRung } from '../../api/client';
import { decodePair, encodePair } from '../../store/modelSelection';
import type { ControlOption } from '../shared/ControlOptionList';

/** The rung list's rows, in order: the ladder's estimated on rungs, and the
 *  pair in force when the ladder does not hold it. Then come the unestimated on
 *  rungs and a link to the ladder editor. Every ladder surface draws these rows:
 *  the Lucidos Agent and both coding agents. There is no Default row. While
 *  nothing is picked, the ladder's default pair is the row that wears the
 *  checkmark, in its own place in the list.
 *
 *  A row's value is an encoded model and effort, so a pick commits both through
 *  `pickModelSelection`. The ladder link commits nothing. */

/** The row that opens the ladder editor instead of committing. */
export const LADDER_LINK_VALUE = 'ladder-link';

export const RUNG_SECTION = 'Model · from your intelligence ladder';
export const NOT_ESTIMATED_SECTION = 'Not estimated';
export const LADDER_LINK_SECTION = 'Not listed?';
export const LADDER_LINK_LABEL = 'Edit the intelligence ladder';
export const LADDER_LINK_NOTE = 'Settings › Intelligence. Add or remove rungs.';
export const OFF_LADDER_NOTE = 'Not on your ladder';

/** The last row of every list a ladder decides: the rung lists and the
 *  Settings model fields. */
export const LADDER_LINK_ROW: ControlOption = {
  value: LADDER_LINK_VALUE,
  label: LADDER_LINK_LABEL,
  description: LADDER_LINK_NOTE,
  drilldown: true,
  section: LADDER_LINK_SECTION,
};

/** Narrow the rung list to a query, the way the model step narrows: every
 *  term must appear in the row's label. The ladder link always stays, since
 *  a rung the filter cannot find is the case it exists for. */
export function filterRungRows(rows: readonly ControlOption[], query: string): ControlOption[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  return rows.filter((row) => {
    if (row.value === LADDER_LINK_VALUE) return true;
    const hay = row.label.toLowerCase();
    return terms.every((t) => hay.includes(t));
  });
}

/** A model id with its context-window twin folded in: `claude-opus-5-5[1m]`
 *  runs the same rung as `claude-opus-5-5`. */
function withoutContextTwin(model: string): string {
  return model.replace(/\[1m\]$/, '');
}

/** The id a model resolves to, so `opus` and the id a thread recorded name the
 *  same rung. */
function canonicalModel(options: readonly CodingAgentCommandOption[], model: string): string {
  const resolved = options.find((o) => o.value === model)?.resolved_model;
  return withoutContextTwin(resolved ?? model);
}

/** Whether `rung` is the pair `model` at `effort`. */
export function isRung(
  options: readonly CodingAgentCommandOption[],
  rung: CodingAgentLadderRung,
  model: string,
  effort: string | null,
): boolean {
  return canonicalModel(options, rung.model) === canonicalModel(options, model) && rung.effort === effort;
}

export const NO_ESTIMATE_NOTE = 'No estimate yet';

/** The line under a rung: its bands as the engine labels them, in the order
 *  of Settings › Intelligence. A missing band is left out. */
function bandsLine(rung: CodingAgentLadderRung): string {
  const parts = [
    rung.intelligence && `Intelligence ${rung.intelligence.label}`,
    rung.cost && `cost ${rung.cost.label}`,
    rung.time && `time ${rung.time.label}`,
  ].filter((part): part is string => !!part);
  return parts.length > 0 ? parts.join(' · ') : NO_ESTIMATE_NOTE;
}

export interface RungListInput {
  rungs: readonly CodingAgentLadderRung[];
  /** The served model rows, which say what an alias such as `opus` resolves to.
   *  The Lucidos Agent has no aliases, so it passes none. */
  options: readonly CodingAgentCommandOption[];
  /** The pair in force. A null model is nothing picked and nothing recorded,
   *  so the default pair is in force. */
  model: string | null;
  effort: string | null;
  modelLabel: (model: string) => string;
  /** What nothing-picked runs as, when the surface knows it. A started Lucidos
   *  Agent thread has none, since its own memory wins over the account default. */
  defaultPair: { model: string; effort: string | null } | null;
  /** Whether picking `model` opens a further step, the preferred-provider
   *  step, rather than committing at once. */
  opensStep?: (model: string) => boolean;
}

/** The rows, in the order they draw. `current` is the row that gets the
 *  checkmark, or an empty string while the pair in force is unknown. */
export function rungListRows(input: RungListInput): { rows: ControlOption[]; current: string } {
  const { rungs, options } = input;
  const picked = input.model === null || input.model === 'default' ? null : input.model;
  const inForce = picked === null ? input.defaultPair : { model: picked, effort: input.effort };
  const onLadder = inForce === null
    ? null
    : rungs.find((r) => isRung(options, r, inForce.model, inForce.effort)) ?? null;
  const opensStep = input.opensStep ?? (() => false);
  const rungRow = (rung: CodingAgentLadderRung, section: string): ControlOption => ({
    value: encodePair(rung.model, rung.effort),
    label: `${rung.label} · ${rung.effort}`,
    description: bandsLine(rung),
    drilldown: opensStep(rung.model) || undefined,
    section,
  });
  const rows = rungs.filter((r) => r.estimated).map((rung) => rungRow(rung, RUNG_SECTION));
  let current = '';
  if (inForce !== null && onLadder) {
    current = encodePair(onLadder.model, onLadder.effort);
  } else if (inForce !== null) {
    current = encodePair(inForce.model, inForce.effort);
    const name = input.modelLabel(inForce.model);
    rows.push({
      value: current,
      label: inForce.effort ? `${name} · ${inForce.effort}` : name,
      description: OFF_LADDER_NOTE,
      drilldown: opensStep(inForce.model) || undefined,
      section: RUNG_SECTION,
    });
  }
  for (const rung of rungs.filter((r) => !r.estimated)) rows.push(rungRow(rung, NOT_ESTIMATED_SECTION));
  rows.push(LADDER_LINK_ROW);
  return { rows, current };
}

/** The pair a row commits, or `null` for the ladder link. */
export function rungRowPair(value: string): { model: string; effort: string | null } | null {
  return value === LADDER_LINK_VALUE ? null : decodePair(value);
}
