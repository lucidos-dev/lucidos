import { useState, useEffect, useRef } from 'preact/hooks';
import { isTauri } from '../../utils/platform';
import { hidePanelWebview, showPanelWebview } from '../../utils/tauri';

export interface DropdownOption {
  value: string;
  label: string;
}

interface DropdownProps {
  options: DropdownOption[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  class?: string;
  filterable?: boolean;
  /** Allow typing a custom value not in the options list */
  freeText?: boolean;
}

export function Dropdown({ options, value, onChange, disabled, placeholder, class: className, filterable, freeText }: DropdownProps) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState('');
  const [focusedIndex, setFocusedIndex] = useState(-1);
  const [draft, setDraft] = useState(value);
  const ref = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const draftRef = useRef(value);
  const lastCommittedRef = useRef(value);
  const selected = options.find((o) => o.value === value);
  const showFilter = filterable || freeText;

  // Sync draft when external value changes (and we're not actively editing)
  useEffect(() => {
    lastCommittedRef.current = value;
    if (!open) {
      setDraft(value);
      draftRef.current = value;
    }
  }, [value, open]);

  function setDraftValue(v: string) {
    draftRef.current = v;
    setDraft(v);
  }

  function commit(explicit?: string) {
    const next = explicit !== undefined ? explicit : draftRef.current;
    const trimmed = next.trim();
    if (trimmed && trimmed !== lastCommittedRef.current) {
      lastCommittedRef.current = trimmed;
      onChange(trimmed);
    }
    setDraftValue(trimmed || lastCommittedRef.current);
  }

  useEffect(() => {
    if (!open) return;
    if (isTauri()) hidePanelWebview();
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        closeDropdown();
      }
    }
    document.addEventListener('click', handleClick);
    return () => {
      document.removeEventListener('click', handleClick);
      if (isTauri()) showPanelWebview();
    };
  }, [open]);

  useEffect(() => {
    if (open && showFilter) {
      requestAnimationFrame(() => {
        if (freeText) inputRef.current?.focus();
        else filterRef.current?.focus();
      });
    }
  }, [open, showFilter]);

  const filtered = showFilter && filter
    ? options.filter(o => o.label.toLowerCase().includes(filter.toLowerCase()))
    : options;

  function closeDropdown() {
    setOpen(false);
    setFilter('');
    setFocusedIndex(-1);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      if (freeText) setDraftValue(value);
      closeDropdown();
      inputRef.current?.blur();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setFocusedIndex(i => Math.min(i + 1, filtered.length - 1));
      if (!open) setOpen(true);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setFocusedIndex(i => Math.max(i - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (focusedIndex >= 0 && focusedIndex < filtered.length) {
        const picked = filtered[focusedIndex].value;
        if (freeText) commit(picked); else onChange(picked);
        closeDropdown();
        inputRef.current?.blur();
      } else if (freeText && draftRef.current.trim()) {
        commit();
        closeDropdown();
        inputRef.current?.blur();
      }
    }
  }

  return (
    <div class={`dropdown${className ? ` ${className}` : ''}`} ref={ref}>
      {freeText ? (
        <div class="dropdown-input-wrap">
          <input
            ref={inputRef}
            type="text"
            class="dropdown-input"
            value={draft}
            disabled={disabled}
            placeholder={placeholder}
            onFocus={() => { if (!disabled) { setFilter(''); setOpen(true); } }}
            onBlur={() => commit()}
            onInput={(e) => {
              const v = (e.target as HTMLInputElement).value;
              setDraftValue(v);
              setFilter(v);
              setFocusedIndex(-1);
              if (!open) setOpen(true);
            }}
            onKeyDown={handleKeyDown}
          />
          <span class="dropdown-chevron" onClick={() => { if (!disabled) setOpen(!open); }}>{open ? '\u25B4' : '\u25BE'}</span>
        </div>
      ) : (
        <button
          type="button"
          class="dropdown-trigger"
          disabled={disabled}
          onClick={() => !disabled && setOpen(!open)}
        >
          <span class="dropdown-sizer">
            <span class={!selected && placeholder ? 'dropdown-placeholder' : ''}>
              {selected?.label ?? placeholder ?? value}
            </span>
            {options.map(o => (
              <span key={o.value} aria-hidden="true">{o.label}</span>
            ))}
          </span>
          <span class="dropdown-chevron">{open ? '\u25B4' : '\u25BE'}</span>
        </button>
      )}
      {open && (
        <div class="dropdown-menu">
          {filterable && !freeText && (
            <input
              ref={filterRef}
              class="dropdown-filter"
              type="text"
              value={filter}
              onInput={(e) => { setFilter((e.target as HTMLInputElement).value); setFocusedIndex(-1); }}
              onKeyDown={handleKeyDown}
              placeholder="Filter..."
            />
          )}
          {filtered.length === 0 && (
            <div class="dropdown-option dropdown-no-results">No matches</div>
          )}
          {filtered.map((o, i) => (
            <div
              key={o.value}
              class={`dropdown-option${o.value === value ? ' active' : ''}${i === focusedIndex ? ' focused' : ''}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                if (freeText) commit(o.value); else onChange(o.value);
                closeDropdown();
                inputRef.current?.blur();
              }}
            >
              {o.label}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
