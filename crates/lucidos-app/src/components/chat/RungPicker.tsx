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

/** A rung's backends, the preferred-provider step a host opens after the rung
 *  is picked. `current` is the backend in force for that model. */
export interface RungProviderStep {
  label: string;
  options: ControlOption[];
  current: string | null;
}

/** The panel's scrollTop that puts the row's middle on the panel's middle. */
export function centredScrollTop(panel: HTMLElement, row: HTMLElement): number {
  const offset = row.getBoundingClientRect().top - panel.getBoundingClientRect().top;
  return Math.max(0, panel.scrollTop + offset - (panel.clientHeight - row.offsetHeight) / 2);
}

/** The rung list: one tap picks a whole rung. A rung whose model has a real
 *  choice of backend opens the preferred-provider step instead, last, as the
 *  model picker does.
 *
 *  The host owns what a pick means, including the ladder link row. It hands
 *  over the rows from `rungListRows`, and the value that wears the checkmark.
 *
 *  Typing filters the rungs, exactly as the model picker filters models. */
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
  /** A way out of the list, for a host that opened it from a list of its own. */
  back?: { label: string; onBack: () => void };
  /** The text field that held the on-screen keyboard as the host opened. A
   *  finger's keyboard goes back to it, so the prompt never loses focus. */
  keyboardHolder?: HTMLElement | null;
  /** The backends a rung opens, or `null` when it commits at once. */
  providerStep?: (value: string) => RungProviderStep | null;
  /** The rung, plus the backend when the provider step chose one. */
  onPick: (value: string, provider?: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const filter = useSignal('');
  // Latches on the first printable keystroke, as the model picker's does.
  const searching = useSignal(false);
  // Open on the rung in force.
  const highlight = useSignal(selectedOptionIndex(rows, current));
  /** The rung whose backends are showing, or `null` on the rung list. */
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

  // The list opens with the rung in force mid-panel, so its neighbours show on
  // both sides. After that the keyboard only nudges the row into view. The
  // centring writes the panel's own scrollTop: `scrollIntoView` would also pan
  // the page, which moves the prompt bar under an on-screen keyboard.
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

  // Escape steps back from the backends before it closes the list.
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
      // A printable key reaching the LIST reveals the box and types into it.
      // Keys typed INTO the box bubble through here too, and it owns those.
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
