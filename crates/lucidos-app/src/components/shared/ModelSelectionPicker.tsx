import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import {
  clampToOffered, decodePair, encodePair, filterModelRows, type ModelRow,
} from '../../store/modelSelection';
import type { ModelSelection } from '../../hooks/useModelSelection';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';
import { focusIfNeeded, isTextInput } from '../../utils/dom';
import { isTouchDevice } from '../../utils/viewport';
import {
  ControlOptionList, selectedOptionIndex, wrapHighlight, type ControlOption,
} from './ControlOptionList';
import { isTypeaheadKey } from './typeahead';

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
    drilldown: row.tiers.length > 0 || row.providers.length > 0,
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

/** One model's backends. The one it would run on is the current value, and
 *  an unconfigured one says so, since picking it refuses the turn. */
export function providerStepOptions(row: ModelRow): ControlOption[] {
  return row.providers.map((p) => ({
    value: p.value,
    label: p.label,
    description: p.configured ? undefined : 'Not set up',
  }));
}

/** The pair a provider pick commits: the held effort, snapped onto what that
 *  backend accepts, so the request never carries an effort it would refuse. */
export function providerStepCommit(
  row: ModelRow,
  provider: string,
  heldEffort: string | null,
): string {
  const tiers = row.providers.find((p) => p.value === provider)?.tiers ?? row.tiers;
  return encodePair(row.value, clampToOffered(heldEffort, tiers));
}

/** The pair a MODEL row commits on its own, or `null` when it opens a step
 *  instead. Only a model with neither tiers nor a backend choice commits at
 *  step 1. For every other, choosing a model reports nothing, so backing out
 *  changes nothing. */
export function modelStepCommit(row: ModelRow): string | null {
  return row.tiers.length === 0 && row.providers.length === 0
    ? encodePair(row.value, null)
    : null;
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

/** Whether the model step draws its filter box.
 *
 *  Typing brings it out, the way every other dropdown reveals its filter. A
 *  panel opened to click one row should not greet the user with an empty box
 *  and a blinking caret.
 *
 *  A touch device always has it, because a box that waits to be typed into is
 *  a box a finger can never reach. `touch` is the capability, NOT the mobile
 *  width breakpoint: a phone held in landscape is over 768px wide and has no
 *  more keyboard than it had upright. */
export function pickerShowsFilter(opts: { searching: boolean; touch: boolean }): boolean {
  return opts.searching || opts.touch;
}

/** Which element must hold focus, since whatever holds it owns the keystrokes.
 *
 *  The list, until a keystroke starts the search: the list's key handler is
 *  what turns that key into the query. The tier step has no filter, so it is
 *  always the list there.
 *
 *  So the box a touch device always shows OPENS unfocused, and that is the
 *  point: the panel would otherwise be half covered by the on-screen keyboard
 *  before the user has asked to search. A tap on the box is the asking. */
export function pickerFocusTarget(opts: { tierStep: boolean; searching: boolean }): 'filter' | 'list' {
  return opts.searching && !opts.tierStep ? 'filter' : 'list';
}

/** Which step a picker shows for an opened model. */
type OpenStep = { model: string; step: 'tiers' } | { model: string; step: 'providers'; effort: string | null };

let pickerIdCounter = 0;

/** The one picker for a *model selection*, on every surface.
 *
 *  Up to three steps. The first lists MODELS, and the row for the model in
 *  force reads the whole selection while every other reads its name alone.
 *  Picking one opens its reasoning tiers. A model with a real choice of backend
 *  then opens its providers, last, since that is the rarely changed part. Only
 *  the last step reports, so backing out leaves the model exactly as it was.
 *
 *  A model with no tiers and no choice reports on the first step, which is how
 *  image generation stays a one-tap pick.
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
  /** One encoded pair, plus the backend when the provider step chose one. The
   *  host applies the whole selection and closes. */
  onPick: (encoded: string, provider?: string) => void;
}) {
  const filterRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  /** The opened model and which of its steps is showing, or `null` on the
   *  model step. */
  const open = useSignal<OpenStep | null>(null);
  const filter = useSignal('');
  // Latches on the first printable keystroke, and dies with the panel. It is
  // what the filter box waits for, exactly as `Dropdown`'s does.
  const searching = useSignal(false);
  const highlight = useSignal(0);
  // Seeded lazily: `useRef(expr)` evaluates `expr` on every render, so a
  // template in the argument would bump the counter forever and keep only the
  // first value.
  const escapeId = useRef('');
  if (!escapeId.current) escapeId.current = `model-picker-${++pickerIdCounter}`;

  const visible = filterModelRows(selection.rows, filter.value);
  const modelOptions = modelStepOptions(visible, selection, describeModel);
  const openRow = selection.rows.find((r) => r.value === open.value?.model) ?? null;
  const openStep = openRow ? open.value : null;
  const stepOptions = !openStep || !openRow
    ? []
    : openStep.step === 'tiers' ? tierStepOptions(openRow) : providerStepOptions(openRow);
  const rows = openStep ? stepOptions : modelOptions;
  const showsFilter = pickerShowsFilter({
    searching: searching.value, touch: isTouchDevice(),
  });

  // Land on the model in force, so a long registry opens where the user is.
  useEffect(() => {
    highlight.value = selectedOptionIndex(modelOptions, selection.model ?? '');
  }, []);

  // Whichever element owns the keystrokes has to hold focus, so focus follows
  // the step, and follows the filter box the moment typing reveals it.
  //
  // The effect asks twice. A host that places its own panel (the Settings
  // field) opens it `visibility: hidden`, and a hidden element cannot take
  // focus, which the frame-late retry covers. The first call wins on a panel
  // already on screen, which is every reveal, and there the next keystroke is
  // already on its way.
  useEffect(() => {
    const target = pickerFocusTarget({ tierStep: !!openStep, searching: searching.value });
    const ref = target === 'filter' ? filterRef : listRef;
    focusIfNeeded(ref.current);
    requestAnimationFrame(() => focusIfNeeded(ref.current));
  }, [open.value, searching.value]);

  // The model step can overflow the panel, and it opens scrolled part-way down.
  useEffect(() => {
    listRef.current?.querySelector('.control-item-active')?.scrollIntoView({ block: 'nearest' });
  }, [highlight.value, open.value]);

  /** Open a model's tiers, highlighting the one NEAREST the effort in force.
   *  Switching model then keeps the user's usual tier under the cursor. It is
   *  highlighted, never checked: only the pair in force wears the checkmark. */
  function openTiers(row: ModelRow, effort: string | null = selection.effort) {
    open.value = { model: row.value, step: 'tiers' };
    const near = clampToOffered(effort, row.tiers);
    const at = row.tiers.findIndex((t) => t.value === near);
    highlight.value = at >= 0 ? at : 0;
  }

  /** Open a model's backends, holding the effort the tier step chose. The
   *  backend it would run on is highlighted. */
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

  /** One step back from the provider step: the tiers, or the models when the
   *  model has no tiers to go back to. */
  function stepBackFromProviders(row: ModelRow, effort: string | null) {
    if (row.tiers.length > 0) openTiers(row, effort);
    else backToModels();
  }

  /** Where Escape goes, or `null` when it should close the panel. */
  const escapeTarget = !openStep || !openRow
    ? back?.onBack ?? null
    : openStep.step === 'providers'
      ? () => stepBackFromProviders(openRow, openStep.effort)
      : backToModels;

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
  }, [open.value, !!back]);

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
    const row = selection.rows.find((r) => r.value === option.value);
    if (!row) return;
    const commit = modelStepCommit(row);
    if (commit !== null) onPick(commit);
    else if (row.tiers.length > 0) openTiers(row);
    else openProviders(row, null);
  }

  /** Consumes and stops every key it owns. A host panel runs its own handler
   *  over this one, and while the picker is up those keys are the picker's. */
  function handleKeyDown(e: KeyboardEvent) {
    const action = pickerKeyAction(e.key);
    if (action === null) {
      // A printable key reaching the LIST is one the box does not own, so this
      // both reveals the box and types into it. The box takes focus a frame
      // later, and the keys pressed inside that frame land here.
      //
      // The target is the whole gate. Keys typed INTO the box bubble through
      // here too, and the box handles those itself.
      if (openStep || isTextInput(e.target) || !isTypeaheadKey(e)) return;
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
      const row = rows[highlight.value];
      if (row) choose(row);
    } else {
      highlight.value = wrapHighlight(highlight.value, rows.length, action === 'next' ? 1 : -1);
    }
  }

  if (openStep && openRow) {
    const providers = openStep.step === 'providers';
    // Only the selection in force wears the checkmark, as on the tier step.
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
      currentValue={selection.model}
      highlightIndex={highlight.value}
      disabled={disabled}
      listRef={listRef}
      back={back}
      filter={showsFilter ? {
        value: filter.value,
        placeholder: 'Filter models...',
        inputRef: filterRef,
        onInput: (value) => { filter.value = value; highlight.value = 0; },
      } : undefined}
      onKeyDown={handleKeyDown}
      onPick={choose}
      onHighlight={(i) => { highlight.value = i; }}
    />
  );
}
