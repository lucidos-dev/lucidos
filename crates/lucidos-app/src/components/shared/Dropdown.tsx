import { DropdownChevron, dropdownPanelStyle } from './dropdownPresentation';
export { DropdownChevron, dropdownPanelStyle } from './dropdownPresentation';
import { useState, useEffect, useRef } from 'preact/hooks';
import { keepFocusOnPress } from '../../utils/dom';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from './Overlay';
import { SearchField } from './SearchField';
import { SkeletonProvider, SkText } from './Skeleton';
import { filterListItems, useListFilter } from './useListFilter';
import { protectedClassFrom } from './protectedSurface';
import { isTouchLayout, keyboardHolder, returnKeyboard } from './keyboardHandoff';

export interface DropdownOption {
  value: string;
  label: string;
  disabled?: boolean;
  description?: string;
  danger?: boolean;
}

interface DropdownProps {
  options: DropdownOption[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  class?: string;
  freeText?: boolean;
  restoreFocusOnSelect?: boolean;
  markCurrent?: boolean;
  onOpen?: () => void;
}

export function filterDropdownOptions(options: DropdownOption[], query: string): DropdownOption[] {
  return filterListItems(options, query);
}

export function openMenuFocusTarget(opts: {
  freeText: boolean;
  searching: boolean;
  positioned: boolean;
  touch: boolean;
}): 'input' | 'filter' | 'trigger' | null {
  if (opts.freeText) return 'input';
  if (opts.searching) return opts.positioned ? 'filter' : null;
  return opts.touch ? null : 'trigger';
}

export function dropdownMenuClass(trigger: { closest(selector: string): unknown } | null): string {
  const base = trigger?.closest('.form-group') ? 'surface-box dropdown-menu dropdown-menu-field' : 'surface-box dropdown-menu';
  return base + protectedClassFrom(trigger);
}

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

export function Dropdown({
  options,
  value,
  onChange,
  disabled,
  placeholder,
  class: className,
  freeText,
  restoreFocusOnSelect = true,
  markCurrent = true,
  onOpen,
}: DropdownProps) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [focusedIndex, setFocusedIndex] = useState(-1);
  const [draft, setDraft] = useState(value);
  const ref = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const draftRef = useRef(value);
  const lastCommittedRef = useRef(value);
  const keyboardHolderRef = useRef<HTMLElement | null>(null);
  const selected = options.find((o) => o.value === value);
  const open = anchor !== null;
  const pos = useAnchoredPosition(anchor, menuRef);
  const listFilter = useListFilter({
    enabled: !freeText,
    escape: open && !freeText,
    onChange: (query) => setFocusedIndex(nextEnabledIndex(filterDropdownOptions(options, query), -1, +1)),
  });
  const { query, inputRef: filterRef, searching, setQuery: setFilter } = listFilter;
  const filter = query.value;

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

  useHidePanelWebviewWhile(open);

  useEffect(() => {
    if (!open) return;
    const target = openMenuFocusTarget({
      freeText: !!freeText, searching: searching || keyboardHolderRef.current !== null, positioned: pos !== null, touch: isTouchLayout(),
    });
    if (target === 'input') {
      const frame = requestAnimationFrame(() => {
        if (inputRef.current && document.activeElement !== inputRef.current) inputRef.current.focus();
      });
      return () => cancelAnimationFrame(frame);
    }
    const el = target === 'filter' ? filterRef.current : target === 'trigger' ? buttonRef.current : null;
    if (el && document.activeElement !== el) el.focus({ preventScroll: true });
  }, [open, searching, pos, freeText]);

  useEffect(() => {
    if (!open || focusedIndex < 0 || !menuRef.current) return;
    const el = menuRef.current.querySelectorAll('.dropdown-option')[focusedIndex] as HTMLElement | undefined;
    el?.scrollIntoView({ block: 'nearest' });
  }, [focusedIndex, open]);

  const filterOptions = (query: string) => filterDropdownOptions(options, query);
  const filtered = filterOptions(filter);

  function closeDropdown(): void {
    hideMenu();
    restoreKeyboardHolder();
  }

  function hideMenu() {
    setAnchor(null);
    setFilter('');
    setFocusedIndex(-1);
  }

  function restoreKeyboardHolder(): boolean {
    const field = keyboardHolderRef.current;
    keyboardHolderRef.current = null;
    return returnKeyboard(field, menuRef.current);
  }

  function selectFocusedAndClose(): boolean {
    if (focusedIndex >= 0 && focusedIndex < filtered.length) {
      const focused = filtered[focusedIndex];
      if (focused.disabled) return false;
      if (freeText) commit(focused.value); else onChange(focused.value);
      hideMenu();
      const restored = restoreKeyboardHolder();
      inputRef.current?.blur();
      if (restoreFocusOnSelect && !restored) buttonRef.current?.focus();
      return true;
    }
    if (freeText && draftRef.current.trim()) {
      commit();
      closeDropdown();
      inputRef.current?.blur();
      return true;
    }
    return false;
  }

  function showMenu() {
    if (!ref.current) return;
    setAnchor(ref.current);
    onOpen?.();
  }

  function openDropdown() {
    if (!ref.current) return;
    showMenu();
    keyboardHolderRef.current = keyboardHolder();
    if (query.value) return;
    const currentIdx = options.findIndex((o) => o.value === value);
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
    if (e.key === 'Tab' && open && !freeText) {
      e.preventDefault();
      e.stopPropagation();
      if (!selectFocusedAndClose()) closeDropdown();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (!open) openDropdown();
      else setFocusedIndex(i => nextEnabledIndex(filtered, i, +1));
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (!open) openDropdown();
      else setFocusedIndex(i => nextEnabledIndex(filtered, i, -1));
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (!open) openDropdown();
      else selectFocusedAndClose();
      return;
    }
    if (e.key === ' ' && !freeText && !open) {
      e.preventDefault();
      openDropdown();
      return;
    }
    if (!freeText && listFilter.handleKeyDown(e) && !open) openDropdown();
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
            onFocus={() => { if (!disabled) { setFilter(''); showMenu(); } }}
            onBlur={() => commit()}
            onInput={(e) => {
              const v = (e.target as HTMLInputElement).value;
              setDraftValue(v);
              setFilter(v);
              setFocusedIndex(-1);
              if (!open) showMenu();
            }}
            onKeyDown={handleKeyDown}
          />
          <DropdownChevron open={open} onClick={() => {
            if (disabled) return;
            if (open) closeDropdown();
            else showMenu();
          }} />
        </div>
      ) : (
        <button
          ref={buttonRef}
          type="button"
          class="dropdown-trigger"
          disabled={disabled}
          aria-haspopup="listbox"
          aria-expanded={open}
          onMouseDown={keepFocusOnPress}
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
          <DropdownChevron open={open} />
        </button>
      )}
      <Overlay
        open={open}
        onClose={closeDropdown}
        anchor={ref.current}
        backdrop={false}
        portal
        panelClass={dropdownMenuClass(anchor)}
        panelRef={menuRef}
        panelStyle={dropdownPanelStyle(anchor ? anchor.getBoundingClientRect().width : null, pos)}
      >
          {!freeText && listFilter.visible && (
            <div class="dropdown-filter-bar">
              <SearchField
                inputRef={filterRef}
                inputClass="dropdown-filter"
                value={filter}
                onInput={(e) => setFilter(e.currentTarget.value)}
                onKeyDown={handleKeyDown}
                placeholder="Filter…"
              />
            </div>
          )}
          {filtered.length === 0 && (
            <div class="dropdown-option dropdown-no-results">No matches</div>
          )}
          {filtered.map((o, i) => (
            <div
              key={o.value}
              class={`dropdown-option${markCurrent && o.value === value ? ' active' : ''}${i === focusedIndex ? ' focused' : ''}${o.disabled && !o.danger ? ' dropdown-option-header' : ''}${o.danger ? ' dropdown-option-danger' : ''}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                if (o.disabled) return;
                if (freeText) commit(o.value); else onChange(o.value);
                closeDropdown();
                inputRef.current?.blur();
              }}
            >
              <div class="dropdown-option-label">{o.label}</div>
              {o.description !== undefined && (
                <div class="dropdown-option-description">{o.description}</div>
              )}
            </div>
          ))}
      </Overlay>
    </div>
  );
}

export function DropdownSkeleton({ w }: { w: string }) {
  return (
    <SkeletonProvider>
      <span class="dropdown-trigger dropdown-skeleton" aria-hidden="true">
        <SkText w={w} />
        <DropdownChevron open={false} />
      </span>
    </SkeletonProvider>
  );
}
