import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import {
  clampToOffered, encodePair, filterModelRows, type ModelRow,
} from '../../store/modelSelection';
import type { ModelSelection } from '../../hooks/useModelSelection';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';
import { focusIfNeeded } from '../../utils/dom';
import {
  ControlOptionList, selectedOptionIndex, wrapHighlight, type ControlOption,
} from './ControlOptionList';

/** The MODEL step's rows.
 *
 *  The model in force reads the whole pair, and every other reads its own name.
 *  So the list says what is selected, without printing a tier on thirty rows
 *  that do not have one. A model with tiers shows the disclosure, because
 *  picking it opens a list rather than committing. */
export function modelStepOptions(
  rows: readonly ModelRow[],
  current: { model: string | null; label: string },
  describe?: (row: ModelRow) => string | undefined,
): ControlOption[] {
  return rows.map((row) => ({
    value: row.value,
    label: row.value === current.model ? current.label : row.label,
    description: describe ? describe(row) : row.description,
    drilldown: row.tiers.length > 0,
  }));
}

/** One model's tiers, each carrying the encoded pair it commits. */
export function tierStepOptions(row: ModelRow): ControlOption[] {
  return row.tiers.map((tier) => ({
    value: encodePair(row.value, tier.value),
    label: tier.label,
    description: tier.description,
  }));
}

/** The pair a MODEL row commits on its own, or `null` when it opens a tier
 *  step instead. Only a tierless model commits at step 1. For every other,
 *  choosing a model reports nothing, so backing out changes nothing. */
export function modelStepCommit(row: ModelRow): string | null {
  return row.tiers.length === 0 ? encodePair(row.value, null) : null;
}

/** What a keystroke means to the picker, or `null` to leave it alone.
 *
 *  Escape is deliberately absent. It belongs to the central overlay stack, not
 *  to a keydown handler: the Escape dispatcher runs in the CAPTURE phase and
 *  stops propagation, so no element's own handler ever sees the key. */
export function pickerKeyAction(key: string): 'choose' | 'next' | 'prev' | null {
  if (key === 'Enter') return 'choose';
  if (key === 'ArrowDown') return 'next';
  if (key === 'ArrowUp') return 'prev';
  return null;
}

let pickerIdCounter = 0;

/** The one picker for a *model selection*, on every surface.
 *
 *  Two steps. The first lists MODELS, and the row for the model in force reads
 *  the whole pair while every other reads its name alone. Picking one opens its
 *  reasoning tiers, and choosing there reports the pair. So nothing is written
 *  between the steps: backing out leaves the model exactly as it was.
 *
 *  A model with no tiers has no second step and reports on the first, which is
 *  how image generation stays a one-tap pick.
 *
 *  Every host mounts this same body: both prompt-bar control menus, the
 *  Settings field and the trigger form. A host supplies only a trigger, an
 *  overlay and what cancelling means. */
export function ModelSelectionPicker({
  label,
  selection,
  disabled,
  describeModel,
  back,
  onPick,
}: {
  /** The section label over the model step. The tier step names the model. */
  label: string;
  selection: ModelSelection;
  disabled?: boolean;
  /** Override a model row's muted note. The coding-agent menu uses it to say
   *  what its Default row currently resolves to. */
  describeModel?: (row: ModelRow) => string | undefined;
  /** A way out of the MODEL step, for a host that opened the picker from a
   *  list of its own. Omit it where the picker IS the panel. */
  back?: { label: string; onBack: () => void };
  /** One encoded pair, the whole selection. The host applies it and closes. */
  onPick: (encoded: string) => void;
}) {
  const filterRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  /** The model whose tiers are showing, or `null` on the model step. */
  const openModel = useSignal<string | null>(null);
  const filter = useSignal('');
  const highlight = useSignal(0);
  // Seeded lazily: `useRef(expr)` evaluates `expr` on every render, so a
  // template in the argument would bump the counter forever and keep only the
  // first value.
  const escapeId = useRef('');
  if (!escapeId.current) escapeId.current = `model-picker-${++pickerIdCounter}`;

  const visible = filterModelRows(selection.rows, filter.value);
  const modelOptions = modelStepOptions(visible, selection, describeModel);
  const openRow = selection.rows.find((r) => r.value === openModel.value) ?? null;
  const tierOptions = openRow ? tierStepOptions(openRow) : [];
  const rows = openRow ? tierOptions : modelOptions;

  // Land on the model in force, so a long registry opens where the user is.
  useEffect(() => {
    highlight.value = selectedOptionIndex(modelOptions, selection.model ?? '');
  }, []);

  // Whichever step is showing owns the keystrokes, so focus has to follow it.
  useEffect(() => {
    if (openRow) focusIfNeeded(listRef.current);
    else requestAnimationFrame(() => focusIfNeeded(filterRef.current));
  }, [openModel.value]);

  // The model step can overflow the panel, and it opens scrolled part-way down.
  useEffect(() => {
    listRef.current?.querySelector('.control-item-active')?.scrollIntoView({ block: 'nearest' });
  }, [highlight.value, openModel.value]);

  /** Open a model's tiers, highlighting the one NEAREST the effort in force.
   *  Switching model then keeps the user's usual tier under the cursor. It is
   *  highlighted, never checked: only the pair in force wears the checkmark. */
  function openTiers(row: ModelRow) {
    openModel.value = row.value;
    const near = clampToOffered(selection.effort, row.tiers);
    const at = row.tiers.findIndex((t) => t.value === near);
    highlight.value = at >= 0 ? at : 0;
  }

  function backToModels() {
    const from = openModel.value;
    openModel.value = null;
    highlight.value = selectedOptionIndex(visible, from ?? selection.model ?? '');
  }

  /** Where Escape goes, or `null` when it should close the panel. */
  const escapeTarget = openRow ? backToModels : back?.onBack ?? null;

  // Escape must step BACK before it closes, and only the central overlay stack
  // can express that: the Escape dispatcher runs in the capture phase and stops
  // propagation, so a keydown handler here would never see the key.
  //
  // The stack is LIFO, so this entry must be pushed AFTER the panel's own, and
  // both cases satisfy that structurally. The tier step is entered by a click,
  // long after the panel opened. A host passing `back` opened this picker from
  // a list of its own, so its panel was already open too.
  const stepBack = useRef(escapeTarget);
  stepBack.current = escapeTarget;
  useEffect(() => {
    const id = escapeId.current;
    if (stepBack.current === null) return;
    pushOverlay({ id, dismiss: () => stepBack.current?.(), hasPanel: false });
    return () => removeOverlay(id);
  }, [openModel.value, !!back]);

  function choose(option: ControlOption) {
    if (openRow) {
      onPick(option.value);
      return;
    }
    const row = selection.rows.find((r) => r.value === option.value);
    if (!row) return;
    const commit = modelStepCommit(row);
    if (commit === null) openTiers(row);
    else onPick(commit);
  }

  /** Consumes and stops every key it owns. A host panel runs its own handler
   *  over this one, and while the picker is up those keys are the picker's. */
  function handleKeyDown(e: KeyboardEvent) {
    const action = pickerKeyAction(e.key);
    if (action === null) return;
    e.preventDefault();
    e.stopPropagation();
    if (action === 'choose') {
      const row = rows[highlight.value];
      if (row) choose(row);
    } else {
      highlight.value = wrapHighlight(highlight.value, rows.length, action === 'next' ? 1 : -1);
    }
  }

  if (openRow) {
    return (
      <ControlOptionList
        label={openRow.label}
        options={tierOptions}
        currentValue={selection.value}
        highlightIndex={highlight.value}
        disabled={disabled}
        listRef={listRef}
        back={{ label: 'All models', onBack: backToModels }}
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
      currentValue={selection.model}
      highlightIndex={highlight.value}
      disabled={disabled}
      listRef={listRef}
      back={back}
      filter={{
        value: filter.value,
        placeholder: 'Filter models...',
        inputRef: filterRef,
        onInput: (value) => { filter.value = value; highlight.value = 0; },
      }}
      onKeyDown={handleKeyDown}
      onPick={choose}
      onHighlight={(i) => { highlight.value = i; }}
    />
  );
}
