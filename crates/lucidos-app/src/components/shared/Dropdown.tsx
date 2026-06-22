import { useState, useEffect, useRef } from 'preact/hooks';
import { isTauri } from '../../utils/platform';
import { isMobile } from '../../utils/viewport';
import { hidePanelWebview, showPanelWebview } from '../../utils/tauri';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { Overlay } from './Overlay';

export interface DropdownOption {
  value: string;
  label: string;
  /** Non-selectable section header. Renders dimmer + ignores click / keyboard
   *  select. Use to group options inside a flat list (e.g. the compose
   *  destination picker grouping coding targets under "Coding agent on…"). */
  disabled?: boolean;
  /** Optional second line rendered muted under the label in the open menu.
   *  Not part of the trigger sizing — only labels feed `.dropdown-sizer`. */
  description?: string;
  /** Error styling for a (usually disabled) row — a failed load must look
   *  different from an empty group (frontend.md "No Hidden Errors"). */
  danger?: boolean;
}

interface DropdownProps {
  options: DropdownOption[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  class?: string;
  /** Allow typing a custom value not in the options list */
  freeText?: boolean;
  /** Keyboard selection usually restores focus to the trigger for accessibility.
   *  Callers that synchronously focus a follow-up input from onChange can opt
   *  out so the trigger does not steal focus back. */
  restoreFocusOnSelect?: boolean;
}

/** Filter options by a case-insensitive label substring. Empty query → the full
 *  list unchanged. Pure — exported for unit testing the type-to-search behavior. */
export function filterDropdownOptions(options: DropdownOption[], query: string): DropdownOption[] {
  const q = query.trim().toLowerCase();
  return q ? options.filter(o => o.label.toLowerCase().includes(q)) : options;
}

/** Whether a keydown should START type-to-search (reveal + seed the filter box)
 *  rather than navigate/select: a single printable char (not Space), no
 *  modifiers, on a non-freeText dropdown that isn't already searching. Once
 *  searching, the focused filter input owns subsequent input, so this returns
 *  false and the keystroke flows into it natively. Pure — exported for testing. */
export function isTypeaheadSeedKey(
  e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
  opts: { freeText: boolean; searching: boolean },
): boolean {
  return !opts.freeText && !opts.searching
    && e.key.length === 1 && e.key !== ' '
    && !e.metaKey && !e.ctrlKey && !e.altKey;
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

export function Dropdown({
  options,
  value,
  onChange,
  disabled,
  placeholder,
  class: className,
  freeText,
  restoreFocusOnSelect = true,
}: DropdownProps) {
  // Anchor element when open, null when closed. `useAnchoredPosition` reacts
  // to anchor changes via its effect deps — no separate `open` flag needed.
  // The menu uses `position: fixed` so it escapes any `overflow: hidden`
  // ancestor (notably `.mobile-swipe-pane` on mobile, where an
  // `absolute`-positioned menu was clipped at the pane edge regardless of
  // placement direction).
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [filter, setFilter] = useState('');
  // Latches true on the first printable keystroke while open and resets on
  // close. Gates the filter box: it stays hidden (and unfocused — no blinking
  // caret) until the user actually types, then appears and owns input.
  const [searching, setSearching] = useState(false);
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

  useEffect(() => {
    if (!open || !isTauri()) return;
    hidePanelWebview();
    return () => showPanelWebview();
  }, [open]);

  // Focus management while open. For freeText the trigger IS the text input, so
  // focus it on open (existing behavior). For a normal dropdown, focus stays on
  // the trigger button until the user types — so there's no empty filter box and
  // no blinking caret on open. Once `searching` latches, move focus into the now-
  // visible filter box (desktop only — auto-focusing on mobile would pop the
  // on-screen keyboard; and only once `pos` is computed, since the panel is
  // `visibility: hidden` — and unfocusable — until then).
  useEffect(() => {
    if (!open) return;
    if (freeText) {
      requestAnimationFrame(() => {
        if (inputRef.current && document.activeElement !== inputRef.current) inputRef.current.focus();
      });
    } else if (searching && pos && !isMobile() && filterRef.current
      && document.activeElement !== filterRef.current) {
      filterRef.current.focus();
    }
  }, [open, searching, pos, freeText]);

  useEffect(() => {
    if (!open || focusedIndex < 0 || !menuRef.current) return;
    const el = menuRef.current.querySelectorAll('.dropdown-option')[focusedIndex] as HTMLElement | undefined;
    el?.scrollIntoView({ block: 'nearest' });
  }, [focusedIndex, open]);

  // Every dropdown is searchable: typing while open filters the option list by
  // label (case-insensitive substring) via the shared `filterDropdownOptions`.
  const filterOptions = (query: string) => filterDropdownOptions(options, query);
  const filtered = filterOptions(filter);

  function closeDropdown() {
    setAnchor(null);
    setFilter('');
    setSearching(false);
    setFocusedIndex(-1);
  }

  /** Commit the focused option (or the typed freeText draft) and close. Returns
   *  true iff something was selected — Tab uses this and falls back to a plain
   *  close when there's nothing valid to pick. */
  function selectFocusedAndClose(): boolean {
    if (focusedIndex >= 0 && focusedIndex < filtered.length) {
      const focused = filtered[focusedIndex];
      if (focused.disabled) return false;
      if (freeText) commit(focused.value); else onChange(focused.value);
      closeDropdown();
      inputRef.current?.blur();
      if (restoreFocusOnSelect) buttonRef.current?.focus();
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
    // Tab on an open menu commits the highlighted option and closes — and is
    // swallowed so it neither tabs focus away nor reaches the global Tab trap.
    // freeText keeps its native Tab (move on; its onBlur commits the draft).
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
    if (e.key === ' ' && !freeText) {
      // Space opens a closed menu (like a native <select>/button). While open
      // but not yet searching, swallow it so the trigger button's default
      // space→click doesn't close the menu; once searching it falls through to
      // the focused filter box as a literal space. (freeText is exempt — its
      // trigger is a text input and must accept spaces, e.g. cron expressions.)
      if (!open) { e.preventDefault(); openDropdown(); return; }
      if (!searching) e.preventDefault();
      return;
    }
    // Typeahead: a printable key while not yet searching (focus is on the
    // trigger button, not the filter box) opens the menu if needed, reveals the
    // filter box, and seeds it. stopPropagation keeps this first keystroke from
    // the global type-to-focus handler (which would write it into the prompt).
    // Once searching, the focused filter input owns subsequent keystrokes.
    if (isTypeaheadSeedKey(e, { freeText: !!freeText, searching })) {
      e.preventDefault();
      e.stopPropagation();
      if (!open) openDropdown();
      setSearching(true);
      setFilter(e.key);
      setFocusedIndex(nextEnabledIndex(filterOptions(e.key), -1, +1));
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
          aria-haspopup="listbox"
          aria-expanded={open}
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
      <Overlay
        open={open}
        onClose={closeDropdown}
        anchor={ref.current}
        backdrop={false}
        panelClass="dropdown-menu"
        panelRef={menuRef}
        panelStyle={anchor && pos
          ? {
              position: 'fixed',
              top: `${pos.top}px`,
              left: `${pos.left}px`,
              minWidth: `${anchor.getBoundingClientRect().width}px`,
            }
          : { visibility: 'hidden' }}
      >
          {!freeText && searching && (
            <input
              ref={filterRef}
              class="dropdown-filter"
              type="text"
              value={filter}
              onInput={(e) => {
                const v = (e.target as HTMLInputElement).value;
                setFilter(v);
                // Highlight the first match so Enter picks the top result.
                setFocusedIndex(nextEnabledIndex(filterOptions(v), -1, +1));
              }}
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
              // `danger` suppresses the header look (uppercase/letter-spacing)
              // even when the row is also `disabled` — an error row is prose,
              // not a section heading.
              class={`dropdown-option${o.value === value ? ' active' : ''}${i === focusedIndex ? ' focused' : ''}${o.disabled && !o.danger ? ' dropdown-option-header' : ''}${o.danger ? ' dropdown-option-danger' : ''}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                if (o.disabled) return;
                if (freeText) commit(o.value); else onChange(o.value);
                closeDropdown();
                inputRef.current?.blur();
              }}
            >
              {/* One DOM shape for every option: label always wrapped, so
                  per-caller ellipsis/styling rules don't fork on whether a
                  description is present. */}
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
