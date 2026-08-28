import { useState, useEffect, useRef } from 'preact/hooks';
import type { JSX } from 'preact';
import { isMobile } from '../../utils/viewport';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from './Overlay';
import { SkeletonProvider, SkText } from './Skeleton';
import { isTypeaheadSeedKey } from './typeahead';

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
  /** Mark the option matching `value` as the current selection (a subtle dot —
   *  see `.dropdown-option.active`). Default `true` for dropdowns that reflect a
   *  persisted current setting. Set `false` where the value is a transient
   *  "last used" choice that's about to be replaced (the compose destination +
   *  coding-agent pickers) — there, marking it just competes with the arrow-key
   *  `.focused` highlight and tells the user nothing useful. */
  markCurrent?: boolean;
  /** Fired every time the menu opens (click, Enter/Space, arrow key, typeahead,
   *  or a freeText focus), never on close. For an option list that can go stale
   *  or fail to load: refresh — or retry — it from the user's own gesture,
   *  instead of leaving a dead list on screen. Must be cheap and idempotent;
   *  loaders that single-flight (`loadRepositories`, `loadApps`) are the fit. */
  onOpen?: () => void;
}

/** Filter options by a case-insensitive label substring. Empty query → the full
 *  list unchanged. Pure — exported for unit testing the type-to-search behavior. */
export function filterDropdownOptions(options: DropdownOption[], query: string): DropdownOption[] {
  const q = query.trim().toLowerCase();
  return q ? options.filter(o => o.label.toLowerCase().includes(q)) : options;
}

/** Which element must hold focus while the menu is open. Whatever has focus
 *  handles the menu's keystrokes. That is what makes type-to-search work:
 *  `'trigger'` routes a printable key into the typeahead seed, and `'filter'`
 *  owns input once that seed has revealed the box.
 *
 *  The open effect ASSERTS `'trigger'` rather than assuming it. WebKit does not
 *  focus a `<button>` on click (the macOS convention), so a menu opened with
 *  the mouse left focus where it already was. In the Tauri app that is usually
 *  the prompt textarea, where the filter query landed instead of filtering.
 *
 *  `null` leaves focus alone, in two cases. On mobile, moving it would only pop
 *  the on-screen keyboard, and there is no keyboard to type-to-search with.
 *  Before the panel is positioned it is `visibility: hidden`, and unfocusable.
 *  Pure, exported for testing. */
export function openMenuFocusTarget(opts: {
  freeText: boolean;
  searching: boolean;
  positioned: boolean;
  mobile: boolean;
}): 'input' | 'filter' | 'trigger' | null {
  if (opts.freeText) return 'input';
  if (opts.mobile) return null;
  if (opts.searching) return opts.positioned ? 'filter' : null;
  return 'trigger';
}

/** Class list for the menu panel. The menu is portaled to `<body>` (see the
 *  `<Overlay portal>` below), which severs every ancestor-scoped rule that used
 *  to reach it through the DOM. Exactly one such context styled the rows: a
 *  dropdown used as a **form field**, whose taller `.form-group` control wants
 *  its options sized to match. That context travels with the panel as its own
 *  class (`.dropdown-menu-field`) instead of being read off an ancestor that is
 *  no longer there. Pure over anything `closest`-capable, so it is testable
 *  without a DOM. */
export function dropdownMenuClass(trigger: { closest(selector: string): unknown } | null): string {
  return trigger?.closest('.form-group') ? 'dropdown-menu dropdown-menu-field' : 'dropdown-menu';
}

/** Inline style for the menu panel, given the trigger's width and the computed
 *  position (`null` until `useAnchoredPosition` has measured the panel).
 *
 *  `minWidth` is present from the FIRST render, before `pos` exists, and that
 *  is load-bearing rather than tidy: the hook measures THIS panel to compute
 *  the position, and the panel is portaled to `<body>`, where the stylesheet's
 *  `min-width: 100%` resolves against the initial containing block. Left to
 *  the stylesheet, the measurement pass would report a viewport-wide menu, and
 *  `computeAnchorPosition` would strand `left` at the viewport margin instead
 *  of under the trigger, jumping into place a frame or two later once the
 *  ResizeObserver re-measured the narrowed panel. (`.thread-overflow-menu`
 *  hit the same trap from the other direction and fixed it with
 *  `width: max-content`; a min-width would have beaten that, so this one is
 *  answered where it is set.) `fixed` plus the zeroed offsets keep the hidden
 *  measurement box inside the viewport rather than 100vh down the document,
 *  so it measures at exactly the geometry it will be shown at. */
export function dropdownPanelStyle(
  anchorWidth: number | null,
  pos: { top: number; left: number } | null,
): JSX.CSSProperties {
  if (anchorWidth === null) return { visibility: 'hidden' };
  return {
    position: 'fixed',
    minWidth: `${anchorWidth}px`,
    ...(pos
      ? { top: `${pos.top}px`, left: `${pos.left}px` }
      : { top: '0px', left: '0px', visibility: 'hidden' }),
  };
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
  markCurrent = true,
  onOpen,
}: DropdownProps) {
  // Anchor element when open, null when closed. `useAnchoredPosition` reacts
  // to anchor changes via its effect deps — no separate `open` flag needed.
  // The menu uses `position: fixed` (and is portaled to <body>, see the
  // `<Overlay>` below) so it escapes any `overflow: hidden` ancestor (notably
  // `.mobile-swipe-pane` on mobile, where an `absolute`-positioned menu was
  // clipped at the pane edge regardless of placement direction).
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

  useHidePanelWebviewWhile(open);

  // Focus management while open. `openMenuFocusTarget` holds the policy; this
  // effect applies it. The freeText input keeps its one-frame deferral, since
  // its own `onFocus` is one of the paths that opens the menu.
  useEffect(() => {
    if (!open) return;
    const target = openMenuFocusTarget({
      freeText: !!freeText, searching, positioned: pos !== null, mobile: isMobile(),
    });
    if (target === 'input') {
      requestAnimationFrame(() => {
        if (inputRef.current && document.activeElement !== inputRef.current) inputRef.current.focus();
      });
      return;
    }
    const el = target === 'filter' ? filterRef.current : target === 'trigger' ? buttonRef.current : null;
    // preventScroll: the trigger was just activated, so it is already in view,
    // and scrolling to it would shift the surface under the open menu.
    if (el && document.activeElement !== el) el.focus({ preventScroll: true });
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

  /** Single door into the open state — every open path goes through here so
   *  `onOpen` can't be missed by one of them. */
  function showMenu() {
    if (!ref.current) return;
    setAnchor(ref.current);
    onOpen?.();
  }

  function openDropdown() {
    if (!ref.current) return;
    showMenu();
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
          <span class="dropdown-chevron" onClick={() => {
            if (disabled) return;
            if (open) closeDropdown();
            else showMenu();
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
        // Portaled to <body>, and both halves of that matter. The menu carries
        // VIEWPORT coordinates, so a `transform`ed ancestor would become its
        // containing block and resolve them against the wrong origin; and the
        // pane it opens from is a stacking context (`.mobile-swipe-pane` is
        // `isolation: isolate` + `translateZ(0)`), which caps every z-index
        // inside it below the floating header chrome however high the menu
        // asks to be. Hoisting the panel out of `.app-shell` is what lets the
        // z-index in `.dropdown-menu` actually out-rank that chrome.
        portal
        panelClass={dropdownMenuClass(anchor)}
        panelRef={menuRef}
        panelStyle={dropdownPanelStyle(anchor ? anchor.getBoundingClientRect().width : null, pos)}
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
              class={`dropdown-option${markCurrent && o.value === value ? ' active' : ''}${i === focusedIndex ? ' focused' : ''}${o.disabled && !o.danger ? ' dropdown-option-header' : ''}${o.danger ? ' dropdown-option-danger' : ''}`}
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

/**
 * A dropdown-shaped loading skeleton, for a control whose value is still in flight.
 *
 * It lives here, beside the real thing, and it wears the trigger's OWN
 * `.dropdown-trigger` box rather than a hand-sized `SkBlock`: padding, border,
 * radius, the flex gap and the font metrics then come from one rule, so the
 * skeleton is the size of the control that replaces it and the row does not jump
 * on settle. A `SkBlock` measured by eye is right only until someone re-pads
 * the trigger.
 *
 * The chevron is drawn as its real glyph for the same reason. Everything about
 * its footprint (`.dropdown-chevron`'s own smaller font size, and the gap the
 * trigger puts before it) is then the trigger's rule rather than a second
 * guess, and the skeleton is not narrower than the control by exactly a chevron.
 * It is the one part shown rather than shimmered: a skeleton is meant to read as
 * the shape of what is coming, and this part of the shape is already known.
 *
 * Only the LABEL is guessed. `w` is the width of the widest label the slot will
 * show, since nothing can know that before the options land.
 *
 * Gate it behind `useDelayedFlag` and wrap it in `<LoadingFade>` like every
 * other skeleton (`.claude/rules/frontend.md`), so a fast load never shows it
 * and a slow one crossfades out instead of snapping.
 */
export function DropdownSkeleton({ w }: { w: string }) {
  return (
    <SkeletonProvider>
      {/* A span, not the real <button>: a skeleton must not be focusable or
          announced, and `.dropdown-skeleton` drops the trigger's hover tell. */}
      <span class="dropdown-trigger dropdown-skeleton" aria-hidden="true">
        <SkText w={w} />
        <span class="dropdown-chevron">{'▾'}</span>
      </span>
    </SkeletonProvider>
  );
}
