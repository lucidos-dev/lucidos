import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import { useFocusWhenPlaced } from '../../hooks/useFocusWhenPlaced';
import { whenStepSettled } from '../../hooks/panelStepMorph';
import { hasCoarsePointer, isTouchDevice } from '../../utils/viewport';
import { isTextInput } from '../../utils/dom';
import { ControlOptionList, selectedOptionIndex, wrapHighlight, type ControlOption } from '../shared/ControlOptionList';
import { pickerFocusTarget, pickerKeyAction, pickerShowsFilter } from '../shared/ModelSelectionPicker';
import { isTypeaheadKey } from '../shared/typeahead';
import { filterRungRows } from './rungRows';

export interface RungProviderStep {
  label: string;
  options: ControlOption[];
  current: string | null;
}

export function centredScrollTop(panel: HTMLElement, row: HTMLElement): number {
  const offset = row.getBoundingClientRect().top - panel.getBoundingClientRect().top;
  return Math.max(0, panel.scrollTop + offset - (panel.clientHeight - row.offsetHeight) / 2);
}

export function RungPicker({
  label,
  rows,
  current,
  disabled,
  back,
  keyboardHolder = null,
  providerStep,
  onPick,
}: {
  label: string;
  rows: readonly ControlOption[];
  current: string;
  disabled?: boolean;
  back?: { label: string; onBack: () => void };
  keyboardHolder?: HTMLElement | null;
  providerStep?: (value: string) => RungProviderStep | null;
  onPick: (value: string, provider?: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const filter = useSignal('');
  const searching = useSignal(false);
  const highlight = useSignal(selectedOptionIndex(rows, current));
  const openRung = useSignal<string | null>(null);
  const step = openRung.value === null ? null : providerStep?.(openRung.value) ?? null;
  const visible = filterRungRows(rows, filter.value);
  const shownRows = step ? step.options : visible;
  const showsFilter = pickerShowsFilter({
    searching: searching.value, touch: isTouchDevice(), keyboard: keyboardHolder !== null,
  });

  useFocusWhenPlaced(() => {
    const target = pickerFocusTarget({
      tierStep: step !== null, searching: searching.value, touch: hasCoarsePointer(),
      keyboard: keyboardHolder !== null,
    });
    if (target === 'holder') return keyboardHolder;
    if (target === 'filter') return filterRef.current;
    return target === 'list' ? listRef.current : null;
  }, [openRung.value, searching.value]);

  const placed = useRef(false);
  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLElement>('.control-item-active');
    const panel = row?.closest<HTMLElement>('.control-dropdown');
    const centre = !placed.current;
    placed.current = true;
    whenStepSettled(row, () => {
      if (!row) return;
      if (centre && panel) panel.scrollTop = centredScrollTop(panel, row);
      else row.scrollIntoView({ block: 'nearest' });
    });
  }, [highlight.value, openRung.value]);

  function backToRungs() {
    highlight.value = Math.max(0, visible.findIndex((r) => r.value === openRung.value));
    openRung.value = null;
  }

  useEscapeStep(step ? backToRungs : back?.onBack ?? null);

  function choose(option: ControlOption) {
    if (step && openRung.value !== null) {
      onPick(openRung.value, option.value);
      return;
    }
    const opened = providerStep?.(option.value) ?? null;
    if (!opened) {
      onPick(option.value);
      return;
    }
    openRung.value = option.value;
    highlight.value = Math.max(0, opened.options.findIndex((o) => o.value === opened.current));
  }

  function handleKeyDown(e: KeyboardEvent) {
    const action = pickerKeyAction(e.key);
    if (action === null) {
      if (step || isTextInput(e.target) || !isTypeaheadKey(e)) return;
      e.preventDefault();
      e.stopPropagation();
      searching.value = true;
      filter.value += e.key;
      highlight.value = 0;
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (action === 'choose') {
      const row = shownRows[highlight.value];
      if (row) choose(row);
    } else {
      highlight.value = wrapHighlight(highlight.value, shownRows.length, action === 'next' ? 1 : -1);
    }
  }

  if (step) {
    return (
      <ControlOptionList
        label={step.label}
        options={step.options}
        currentValue={step.current}
        highlightIndex={highlight.value}
        disabled={disabled}
        listRef={listRef}
        back={{ label: 'Rungs', onBack: backToRungs }}
        onKeyDown={handleKeyDown}
        onPick={choose}
        onHighlight={(i) => { highlight.value = i; }}
      />
    );
  }

  return (
    <ControlOptionList
      label={label}
      options={visible}
      currentValue={current}
      highlightIndex={highlight.value}
      disabled={disabled}
      listRef={listRef}
      back={back}
      filter={showsFilter ? {
        value: filter.value,
        placeholder: 'Filter models…',
        inputRef: filterRef,
        onInput: (value) => { filter.value = value; highlight.value = 0; },
      } : undefined}
      onKeyDown={handleKeyDown}
      onPick={choose}
      onHighlight={(i) => { highlight.value = i; }}
    />
  );
}
