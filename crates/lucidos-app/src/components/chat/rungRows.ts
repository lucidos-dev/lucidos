import type { CodingAgentCommandOption, CodingAgentLadderRung } from '../../api/client';
import { decodePair, encodePair } from '../../store/modelSelection';
import type { ControlOption } from '../shared/ControlOptionList';


export const LADDER_LINK_VALUE = 'ladder-link';

export const RUNG_SECTION = 'Model · from your intelligence ladder';
export const NOT_ESTIMATED_SECTION = 'Not estimated';
export const LADDER_LINK_SECTION = 'Not listed?';
export const LADDER_LINK_LABEL = 'Edit the intelligence ladder';
export const LADDER_LINK_NOTE = 'Settings › Intelligence. Add or remove rungs.';
export const OFF_LADDER_NOTE = 'Not on your ladder';

export const LADDER_LINK_ROW: ControlOption = {
  value: LADDER_LINK_VALUE,
  label: LADDER_LINK_LABEL,
  description: LADDER_LINK_NOTE,
  drilldown: true,
  section: LADDER_LINK_SECTION,
};

export function filterRungRows(rows: readonly ControlOption[], query: string): ControlOption[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  return rows.filter((row) => {
    if (row.value === LADDER_LINK_VALUE) return true;
    const hay = row.label.toLowerCase();
    return terms.every((t) => hay.includes(t));
  });
}

function withoutContextTwin(model: string): string {
  return model.replace(/\[1m\]$/, '');
}

function canonicalModel(options: readonly CodingAgentCommandOption[], model: string): string {
  const resolved = options.find((o) => o.value === model)?.resolved_model;
  return withoutContextTwin(resolved ?? model);
}

export function isRung(
  options: readonly CodingAgentCommandOption[],
  rung: CodingAgentLadderRung,
  model: string,
  effort: string | null,
): boolean {
  return canonicalModel(options, rung.model) === canonicalModel(options, model) && rung.effort === effort;
}

export const NO_ESTIMATE_NOTE = 'No estimate yet';

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
  options: readonly CodingAgentCommandOption[];
  model: string | null;
  effort: string | null;
  modelLabel: (model: string) => string;
  defaultPair: { model: string; effort: string | null } | null;
  opensStep?: (model: string) => boolean;
}

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

export function rungRowPair(value: string): { model: string; effort: string | null } | null {
  return value === LADDER_LINK_VALUE ? null : decodePair(value);
}
