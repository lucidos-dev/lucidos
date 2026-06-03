import { useState, useEffect, useRef } from 'preact/hooks';
import { isTauri } from '../../utils/platform';
import { hidePanelWebview, showPanelWebview } from '../../utils/tauri';
import { useDismissOnOutside, useAnchoredPosition } from '../../hooks/useAnchoredPopover';

export interface DropdownOption {
  value: string;
  label: string;
  /** Non-selectable section header. Renders dimmer + ignores click / keyboard
   *  select. Use to group options inside a flat list (e.g. compose-view scope
   *  picker grouping Lucidos / external repos / apps). */
  disabled?: boolean;
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

/** Walk `options` from `start` in direction `step` (±1), skipping any
 *  `disabled` entries (section headers). Used by ArrowDown/Up + the initial
 *  focus seed so navigation never lands stuck on a header. Returns `start`
 *  if no enabled option exists in the chosen direction — caller's clamp
 *  keeps focus where it was. */
function nextEnabledIndex(options: DropdownOption[], start: number, step: 1 | -1): number {
  const len = options.length;
  if (len === 0) return -1;
  let i = start + step;
  while (i >= 0 && i < len) {
    if (!options[i].disabled) return i;
    i += step;
  }
  return start;
}

export function Dropdown({ options, value, onChange, disabled, placeholder, class: className, filterable, freeText }: DropdownProps) {
  // Anchor element when open, null when closed. `useAnchoredPosition` reacts
  // to anchor changes via its effect deps — no separate `open` flag needed.
  // The menu uses `position: fixed` so it escapes any `overflow: hidden`
  // ancestor (notably `.mobile-swipe-pane` on mobile, where an
  // `absolute`-positioned menu was clipped at the pane edge regardless of
  // placement direction).
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [filter, setFilter] = useState('');
  const [focusedIndex, setFocusedIndex] = useState(-1);
  const [draft, setDraft] = useState(value);
  const ref = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const draftRef = useRef(value);
  const lastCommittedRef = useRef(value);
  const selected = options.find((o) => o.value === value);
  const showFilter = filterable || freeText;
  const open = anchor !== null;
  const pos = useAnchoredPosition(anchor, menuRef);

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

  // Trigger button and menu both live inside the wrapper ref, so it acts as
  // both panel and anchor — re-clicking the trigger is "inside" and routes
  // through its own onClick toggle. The Tauri panel-webview hide/show stays
  // as its own effect, scoped to the same open-gate.
  useDismissOnOutside(open, ref, null, closeDropdown);

  useEffect(() => {
    if (!open || !isTauri()) return;
    hidePanelWebview();
    return () => showPanelWebview();
  }, [open]);

  useEffect(() => {
    if (open && showFilter) {
      requestAnimationFrame(() => {
        if (freeText) inputRef.current?.focus();
        else filterRef.current?.focus();
      });
    }
  }, [open, showFilter]);

  useEffect(() => {
    if (!open || focusedIndex < 0 || !menuRef.current) return;
    const el = menuRef.current.querySelectorAll('.dropdown-option')[focusedIndex] as HTMLElement | undefined;
    el?.scrollIntoView({ block: 'nearest' });
  }, [focusedIndex, open]);

  const filtered = showFilter && filter
    ? options.filter(o => o.label.toLowerCase().includes(filter.toLowerCase()))
    : options;

  function closeDropdown() {
    setAnchor(null);
    setFilter('');
    setFocusedIndex(-1);
  }

  function openDropdown() {
    if (!ref.current) return;
    setAnchor(ref.current);
    const currentIdx = options.findIndex((o) => o.value === value);
    // Seed at the saved value, or — when missing/stale or pointing at a
    // disabled row (stale scope that collides with a section header after
    // a re-group) — the first enabled option. Without the disabled-skip,
    // focus could land on a header and ArrowDown would silently dead-key.
    if (currentIdx < 0 || options[currentIdx]?.disabled) {
      setFocusedIndex(nextEnabledIndex(options, -1, +1));
    } else {
      setFocusedIndex(currentIdx);
    }
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
      if (!open) {
        openDropdown();
      } else {
        setFocusedIndex(i => nextEnabledIndex(filtered, i, +1));
      }
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (!open) {
        openDropdown();
      } else {
        setFocusedIndex(i => nextEnabledIndex(filtered, i, -1));
      }
    } else if (e.key === 'Enter' || (e.key === ' ' && !showFilter)) {
      if (!open) {
        e.preventDefault();
        openDropdown();
        return;
      }
      e.preventDefault();
      if (focusedIndex >= 0 && focusedIndex < filtered.length) {
        const focused = filtered[focusedIndex];
        if (focused.disabled) return;
        const picked = focused.value;
        if (freeText) commit(picked); else onChange(picked);
        closeDropdown();
        inputRef.current?.blur();
        buttonRef.current?.focus();
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
            onFocus={() => { if (!disabled && ref.current) { setFilter(''); setAnchor(ref.current); } }}
            onBlur={() => commit()}
            onInput={(e) => {
              const v = (e.target as HTMLInputElement).value;
              setDraftValue(v);
              setFilter(v);
              setFocusedIndex(-1);
              if (!open && ref.current) setAnchor(ref.current);
            }}
            onKeyDown={handleKeyDown}
          />
          <span class="dropdown-chevron" onClick={() => {
            if (disabled) return;
            if (open) closeDropdown();
            else if (ref.current) setAnchor(ref.current);
          }}>{open ? '▴' : '▾'}</span>
        </div>
      ) : (
        <button
          ref={buttonRef}
          type="button"
          class="dropdown-trigger"
          disabled={disabled}
          onClick={() => {
            if (disabled) return;
            if (open) closeDropdown(); else openDropdown();
          }}
          onKeyDown={handleKeyDown}
        >
          <span class="dropdown-sizer">
            <span class={!selected && placeholder ? 'dropdown-placeholder' : ''}>
              {selected?.label ?? placeholder ?? value}
            </span>
            {options.map(o => (
              <span key={o.value} aria-hidden="true">{o.label}</span>
            ))}
          </span>
          <span class="dropdown-chevron">{open ? '▴' : '▾'}</span>
        </button>
      )}
      {anchor && (
        <div
          class="dropdown-menu"
          ref={menuRef}
          style={pos
            ? {
                position: 'fixed',
                top: `${pos.top}px`,
                left: `${pos.left}px`,
                minWidth: `${anchor.getBoundingClientRect().width}px`,
              }
            : { visibility: 'hidden' }}
        >
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
              class={`dropdown-option${o.value === value ? ' active' : ''}${i === focusedIndex ? ' focused' : ''}${o.disabled ? ' dropdown-option-header' : ''}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                if (o.disabled) return;
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
