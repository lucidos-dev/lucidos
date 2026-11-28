import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import {
  clampToOffered, decodePair, encodePair, filterModelRows, type ModelRow,
} from '../../store/modelSelection';
import type { ModelSelection } from '../../hooks/useModelSelection';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import { useFocusWhenPlaced } from '../../hooks/useFocusWhenPlaced';
import { hasCoarsePointer } from '../../utils/layoutMedia';
import {
  ControlOptionList, selectedOptionIndex, wrapHighlight, type ControlOption,
} from './ControlOptionList';
import { listFilterVisible, useListFilter } from './useListFilter';
import { whenStepSettled } from '../../hooks/panelStepMorph';

export function modelStepOptions(
  rows: readonly ModelRow[],
  current: { model: string | null; label: string },
  describe?: (row: ModelRow) => string | undefined,
): ControlOption[] {
  return rows.map((row) => ({
    value: row.value,
    label: row.value === current.model ? current.label : row.label,
    description: describe ? describe(row) : row.description,
    drilldown: row.tiers.length > 0 || row.providers.length > 0,
    section: row.section,
  }));
}

export const RECOMMENDED_TIER_BADGE = 'Recommended';

export function tierStepOptions(
  row: ModelRow,
  defaultLabel?: (model: string) => string,
): ControlOption[] {
  const tiers = row.tiers.map((tier) => ({
    value: encodePair(row.value, tier.value),
    label: tier.label,
    description: tier.description,
    badge: tier.recommended ? RECOMMENDED_TIER_BADGE : undefined,
  }));
  if (!defaultLabel || row.tiers.length === 0) return tiers;
  return [{ value: encodePair(row.value, null), label: defaultLabel(row.value) }, ...tiers];
}

export function providerStepOptions(row: ModelRow): ControlOption[] {
  return row.providers.map((p) => ({
    value: p.value,
    label: p.label,
    description: p.configured ? undefined : 'Not set up',
  }));
}

export function providerStepCommit(
  row: ModelRow,
  provider: string,
  heldEffort: string | null,
): string {
  const tiers = row.providers.find((p) => p.value === provider)?.tiers ?? row.tiers;
  return encodePair(row.value, clampToOffered(heldEffort, tiers));
}

export function modelStepCommit(row: ModelRow): string | null {
  return row.tiers.length === 0 && row.providers.length === 0
    ? encodePair(row.value, null)
    : null;
}

export function pickerKeyAction(key: string): 'choose' | 'next' | 'prev' | null {
  if (key === 'Enter') return 'choose';
  if (key === 'ArrowDown') return 'next';
  if (key === 'ArrowUp') return 'prev';
  return null;
}

export function pickerShowsFilter(opts: { searching: boolean; touch: boolean; keyboard: boolean }): boolean {
  return listFilterVisible({ query: opts.searching ? 'x' : '', coarse: opts.touch, narrow: false }) || opts.keyboard;
}

export function pickerFocusTarget(
  opts: { tierStep: boolean; searching: boolean; touch: boolean; keyboard: boolean },
): 'filter' | 'list' | 'holder' | null {
  if (opts.keyboard && !opts.tierStep) return 'filter';
  if (opts.touch) return opts.keyboard ? 'holder' : null;
  return opts.searching && !opts.tierStep ? 'filter' : 'list';
}

export interface PickerLink {
  option: ControlOption;
  onChoose: () => void;
}

type OpenStep = { model: string; step: 'tiers' } | { model: string; step: 'providers'; effort: string | null };


export function ModelSelectionPicker({
  label,
  selection,
  disabled,
  describeModel,
  back,
  keyboardHolder = null,
  link,
  onPick,
}: {
  label: string;
  selection: ModelSelection;
  disabled?: boolean;
  describeModel?: (row: ModelRow) => string | undefined;
  back?: { label: string; onBack: () => void };
  keyboardHolder?: HTMLElement | null;
  link?: PickerLink;
  onPick: (encoded: string, provider?: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const open = useSignal<OpenStep | null>(null);
  const highlight = useSignal(0);
  const listFilter = useListFilter({ escape: false, onChange: () => { highlight.value = 0; } });
  const { query: filter, inputRef: filterRef, searching } = listFilter;
  const visible = filterModelRows(selection.rows.map((row) => ({
    ...row, description: describeModel ? describeModel(row) : row.description,
  })), filter.value);
  const modelOptions = [
    ...modelStepOptions(visible, selection, describeModel),
    ...(link ? [link.option] : []),
  ];
  const openRow = selection.rows.find((r) => r.value === open.value?.model) ?? null;
  const openStep = openRow ? open.value : null;
  const stepOptions = !openStep || !openRow
    ? []
    : openStep.step === 'tiers'
      ? tierStepOptions(openRow, selection.defaultEffortLabel)
      : providerStepOptions(openRow);
  const rows = openStep ? stepOptions : modelOptions;

  useEffect(() => {
    highlight.value = selectedOptionIndex(modelOptions, selection.model ?? '');
  }, []);

  useFocusWhenPlaced(() => {
    const target = pickerFocusTarget({
      tierStep: !!openStep, searching, touch: hasCoarsePointer(),
      keyboard: keyboardHolder !== null,
    });
    if (target === 'holder') return keyboardHolder;
    if (target === 'filter') return filterRef.current;
    return target === 'list' ? listRef.current : null;
  }, [open.value, searching]);

  useEffect(() => {
    const row = listRef.current?.querySelector('.control-item-active');
    whenStepSettled(row, () => row?.scrollIntoView({ block: 'nearest' }));
  }, [highlight.value, open.value]);

  function openTiers(
    row: ModelRow,
    effort: string | null = selection.effortFor(row.value) ?? selection.effort,
  ) {
    open.value = { model: row.value, step: 'tiers' };
    const near = encodePair(row.value, clampToOffered(effort, row.tiers));
    const at = tierStepOptions(row, selection.defaultEffortLabel).findIndex((o) => o.value === near);
    highlight.value = at >= 0 ? at : 0;
  }

  function openProviders(row: ModelRow, effort: string | null) {
    open.value = { model: row.value, step: 'providers', effort };
    const at = row.providers.findIndex((p) => p.value === row.provider);
    highlight.value = at >= 0 ? at : 0;
  }

  function backToModels() {
    const from = open.value?.model;
    open.value = null;
    highlight.value = selectedOptionIndex(visible, from ?? selection.model ?? '');
  }

  function stepBackFromProviders(row: ModelRow, effort: string | null) {
    if (row.tiers.length > 0) openTiers(row, effort);
    else backToModels();
  }

  const escapeTarget = !openStep || !openRow
    ? searching ? listFilter.clear : back?.onBack ?? null
    : openStep.step === 'providers'
      ? () => stepBackFromProviders(openRow, openStep.effort)
      : backToModels;

  useEscapeStep(escapeTarget);

  function choose(option: ControlOption) {
    if (openStep && openRow) {
      if (openStep.step === 'providers') {
        onPick(providerStepCommit(openRow, option.value, openStep.effort), option.value);
      } else if (openRow.providers.length > 0) {
        openProviders(openRow, decodePair(option.value).effort);
      } else {
        onPick(option.value);
      }
      return;
    }
    if (link && option.value === link.option.value) {
      link.onChoose();
      return;
    }
    const row = selection.rows.find((r) => r.value === option.value);
    if (!row) return;
    const commit = modelStepCommit(row);
    if (commit !== null) onPick(commit);
    else if (row.tiers.length > 0) openTiers(row);
    else openProviders(row, null);
  }

  function handleKeyDown(e: KeyboardEvent) {
    const action = pickerKeyAction(e.key);
    if (action === null) {
      if (!openStep) listFilter.handleKeyDown(e);
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (action === 'choose') {
      const row = rows[highlight.value];
      if (row) choose(row);
    } else {
      highlight.value = wrapHighlight(highlight.value, rows.length, action === 'next' ? 1 : -1);
    }
  }

  if (openStep && openRow) {
    const providers = openStep.step === 'providers';
    const inForce = openRow.value === selection.model;
    return (
      <ControlOptionList
        label={providers ? `${openRow.label} · Provider` : openRow.label}
        options={stepOptions}
        currentValue={providers ? (inForce ? openRow.provider : null) : selection.value}
        highlightIndex={highlight.value}
        disabled={disabled}
        listRef={listRef}
        back={providers && openRow.tiers.length > 0
          ? { label: 'Reasoning', onBack: () => stepBackFromProviders(openRow, openStep.effort) }
          : { label: 'All models', onBack: backToModels }}
        onKeyDown={handleKeyDown}
        onPick={choose}
        onHighlight={(i) => { highlight.value = i; }}
      />
    );
  }

  return (
    <ControlOptionList
      label={label}
      options={modelOptions}
      noMatches={visible.length === 0}
      currentValue={selection.model}
      highlightIndex={highlight.value}
      disabled={disabled}
      listRef={listRef}
      back={back}
      filter={listFilter.visible ? {
        value: filter.value,
        placeholder: 'Filter models…',
        inputRef: filterRef,
        onInput: listFilter.setQuery,
      } : undefined}
      onKeyDown={handleKeyDown}
      onPick={choose}
      onHighlight={(i) => { highlight.value = i; }}
    />
  );
}
