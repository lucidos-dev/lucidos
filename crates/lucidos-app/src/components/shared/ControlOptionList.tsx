import { Fragment, type RefObject } from 'preact';
import { keepFocusOnPress } from '../../utils/dom';
import { SearchField } from './SearchField';

export interface ControlOption {
  value: string;
  label: string;
  description?: string;
  drilldown?: boolean;
  section?: string;
  badge?: string;
}

export function sectionHeadingAt(options: readonly ControlOption[], index: number): string | null {
  const section = options[index]?.section;
  if (!section) return null;
  return index > 0 && options[index - 1].section === section ? null : section;
}

export function wrapHighlight(current: number, count: number, delta: 1 | -1): number {
  if (count <= 0) return 0;
  return (current + delta + count) % count;
}

export function selectedOptionIndex(
  options: readonly { value: string }[],
  currentValue: string,
): number {
  const idx = options.findIndex((o) => o.value === currentValue);
  return idx >= 0 ? idx : 0;
}

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

export function ControlOptionList({
  label,
  options,
  currentValue,
  highlightIndex,
  disabled,
  listRef,
  filter,
  noMatches,
  back,
  onKeyDown,
  onPick,
  onHighlight,
}: {
  label: string;
  noMatches?: boolean;
  options: readonly ControlOption[];
  currentValue: string | null;
  highlightIndex: number;
  disabled?: boolean;
  listRef?: RefObject<HTMLDivElement>;
  filter?: {
    value: string;
    placeholder: string;
    inputRef?: RefObject<HTMLInputElement>;
    onInput: (value: string) => void;
  };
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
      {!options[0]?.section && <div class="control-section-label">{label}</div>}
      {(noMatches ?? options.length === 0) && <div class="control-empty">No matches</div>}
      {options.map((opt, index) => {
        const isCurrent = opt.value === currentValue;
        const heading = sectionHeadingAt(options, index);
        return (
          <Fragment key={opt.value}>
          {heading && <div class="control-section-label">{heading}</div>}
          <button
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
