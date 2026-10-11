import { Fragment, type RefObject } from 'preact';
import { keepFocusOnPress } from '../../utils/dom';
import { SearchField } from './SearchField';

/** One row in a control menu's option list. */
export interface ControlOption {
  value: string;
  label: string;
  description?: string;
  /** Picking this row opens another list rather than committing. Draws the
   *  disclosure glyph, so a model row does not read as a finished choice. */
  drilldown?: boolean;
  /** The heading this row sits under. A list whose rows carry sections shows
   *  each heading where its run begins, in place of the list's own label. */
  section?: string;
  /** A short label drawn as a badge after the row's own label. */
  badge?: string;
}

/** The heading to draw above `options[index]`, or `null` when the row
 *  continues the run above it. */
export function sectionHeadingAt(options: readonly ControlOption[], index: number): string | null {
  const section = options[index]?.section;
  if (!section) return null;
  return index > 0 && options[index - 1].section === section ? null : section;
}

/** Move a highlight index by one step within `[0, count)`, wrapping at both
 *  ends. Down past the last row lands on the first, and up past the first lands
 *  on the last. Returns 0 for an empty list. */
export function wrapHighlight(current: number, count: number, delta: 1 | -1): number {
  if (count <= 0) return 0;
  return (current + delta + count) % count;
}

/** Index of `currentValue` in `options`, or 0 when it is not present. Opening a
 *  list always pre-highlights a valid, sensible row. */
export function selectedOptionIndex(
  options: readonly { value: string }[],
  currentValue: string,
): number {
  const idx = options.findIndex((o) => o.value === currentValue);
  return idx >= 0 ? idx : 0;
}

/** The search box pinned over a control-menu list. */
export function ControlFilter({
  value,
  placeholder,
  inputRef,
  onInput,
}: {
  value: string;
  placeholder: string;
  inputRef?: RefObject<HTMLInputElement>;
  onInput: (value: string) => void;
}) {
  return (
    <div class="control-filter-bar">
      <SearchField
        inputClass="control-filter"
        placeholder={placeholder}
        value={value}
        inputRef={inputRef}
        onInput={(e) => onInput(e.currentTarget.value)}
      />
    </div>
  );
}

/** The control-menu list: a section label over `control-item` buttons, the
 *  current one checked, one highlighted for arrow-key navigation.
 *
 *  Both steps of `ModelSelectionPicker` render through this, as do the
 *  coding-agent menu's other option-bearing commands. It holds no state: the
 *  caller owns the query, the highlight and the keyboard, because those are
 *  what a step transition has to reset. */
export function ControlOptionList({
  label,
  options,
  currentValue,
  highlightIndex,
  disabled,
  listRef,
  filter,
  back,
  onKeyDown,
  onPick,
  onHighlight,
}: {
  label: string;
  /** Already narrowed by `filter`, if there is one: the caller owns the query,
   *  because its arrow keys count the rows that survived it. */
  options: readonly ControlOption[];
  /** The value that gets the checkmark. `null` when nothing is selected yet. */
  currentValue: string | null;
  highlightIndex: number;
  disabled?: boolean;
  listRef?: RefObject<HTMLDivElement>;
  /** A search box over the list. Worth it once the list is long enough that
   *  scrolling stops being an answer, which thirty model names are. */
  filter?: {
    value: string;
    placeholder: string;
    inputRef?: RefObject<HTMLInputElement>;
    onInput: (value: string) => void;
  };
  /** A way back to the list this one was opened from. Not part of the
   *  arrow-key run: Escape is the keyboard route back. */
  back?: { label: string; onBack: () => void };
  onKeyDown?: (e: KeyboardEvent) => void;
  onPick: (option: ControlOption) => void;
  onHighlight: (index: number) => void;
}) {
  return (
    <div class="control-list" tabIndex={0} ref={listRef} onKeyDown={onKeyDown} data-surface-step={label}>
      {filter && <ControlFilter {...filter} />}
      {back && (
        <button class="control-item control-back" onMouseDown={keepFocusOnPress} onClick={back.onBack}>
          <span class="control-back-glyph" aria-hidden="true">&#8249;</span>
          {back.label}
        </button>
      )}
      {!options.some((opt) => opt.section) && <div class="control-section-label">{label}</div>}
      {options.length === 0 && <div class="control-empty">No matches</div>}
      {options.map((opt, index) => {
        const isCurrent = opt.value === currentValue;
        const heading = sectionHeadingAt(options, index);
        return (
          <Fragment key={opt.value}>
          {heading && <div class="control-section-label">{heading}</div>}
          <button
            // A tier row is labelled with its tier alone, so its text does not
            // say which model it belongs to. The value does, and a test that
            // has to pick one exact pair needs it.
            data-value={opt.value}
            class={`control-item control-option${index === highlightIndex ? ' control-item-active' : ''}${isCurrent ? ' control-option-current' : ''}`}
            disabled={disabled}
            onMouseDown={keepFocusOnPress}
            onClick={() => onPick(opt)}
            onMouseEnter={() => onHighlight(index)}
          >
            <span class="control-option-text">
              <span class="control-option-label">
                {isCurrent && <span class="control-checkmark">&#10003;</span>}
                {opt.label}
                {opt.badge && <span class="label label-success control-option-badge">{opt.badge}</span>}
              </span>
              {opt.description && <span class="control-option-desc">{opt.description}</span>}
            </span>
            {opt.drilldown && (
              <span class="control-option-more" aria-hidden="true">&#8250;</span>
            )}
          </button>
          </Fragment>
        );
      })}
    </div>
  );
}
