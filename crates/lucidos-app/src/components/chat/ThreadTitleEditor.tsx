import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { renameThread, suggestTitle } from '../../api/threads';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { viewportIsMobile } from '../../utils/viewport';
import { autoResizeTextarea } from '../../utils/dom';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';

interface Props {
  threadId: string;
  title: string;
}

/** Trim and validate a candidate rename. Returns the value to POST, or null
 *  to skip. `isDirty` distinguishes "user typed something" from a pure-blur
 *  no-op — without it, a blur after a mid-edit ThreadTitleGenerated would
 *  POST the stale editValueRef.current (still holding the pre-SSE title)
 *  back to /api/v1/threads/rename and overwrite the new title. */
export function normalizeRename(
  newValue: string,
  currentTitle: string,
  isDirty: boolean,
): string | null {
  const trimmed = newValue.trim();
  if (!isDirty || !trimmed || trimmed === currentTitle) return null;
  return trimmed;
}

export function ThreadTitleEditor({ threadId, title }: Props) {
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState(title);
  const [suggestion, setSuggestion] = useState<string | null>(null);
  const [suggesting, setSuggesting] = useState(false);
  // The shimmer is a loading indicator, so it is delay-gated like every other
  // one (.claude/rules/frontend.md). A `suggestTitle` that settles inside
  // SPINNER_DELAY_MS (the offline / no-provider reject, which comes back almost
  // immediately) would otherwise paint a shimmer bar for a frame or two and
  // yank it away.
  const showSuggesting = useDelayedFlag(suggesting);
  // Mobile renders a <textarea> (multi-line wrap), desktop an <input> (single-line
  // hug) — both share this ref via the setInputEl callback below.
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement | null>(null);
  const setInputEl = useCallback((el: HTMLInputElement | HTMLTextAreaElement | null) => {
    inputRef.current = el;
  }, []);
  const abortRef = useRef<AbortController | null>(null);
  // Guards against double-save when blur races with an in-flight rename.
  const savingRef = useRef(false);
  // onBlur reads this synchronously to avoid stale-closure / async-batching
  // issues with editValue.
  const editValueRef = useRef(title);
  // Set on first onInput, cleared on startEditing/Escape. save() consults it
  // so a blur fired after the user opened the editor without typing — common
  // when the title prop just changed via SSE — short-circuits before POSTing.
  const dirtyRef = useRef(false);

  useEffect(() => {
    if (!editing) setEditValue(title);
  }, [title, editing]);

  // The overlay input keeps DOM focus after save/escape, so onFocus won't
  // re-fire on the next click — blur explicitly so editing can be re-entered
  // without clicking elsewhere first.
  useEffect(() => {
    if (!editing) inputRef.current?.blur();
  }, [editing]);

  const isMobile = viewportIsMobile.value;

  // The mobile <textarea> grows to fit the wrapped title while editing so the
  // whole text stays visible across multiple lines. When not editing it returns
  // to the inset:0 overlay, so the inline height is cleared to let inset govern.
  useEffect(() => {
    if (!isMobile) return;
    const el = inputRef.current as HTMLTextAreaElement | null;
    if (!el) return;
    if (editing) autoResizeTextarea(el);
    else el.style.height = '';
  }, [editValue, editing, isMobile]);

  useEffect(() => () => { abortRef.current?.abort(); }, []);

  // The input is overlaid (transparent, on top) so real taps hit it directly:
  // native focus opens the iOS keyboard, then onFocus runs this. Synthetic
  // test clicks on the display textarea also reach this via its onClick.
  // Idempotent so the two paths can't fire it twice on a real device.
  const startEditing = useCallback(() => {
    if (editing) return;
    abortRef.current?.abort();
    setSuggestion(null);
    setSuggesting(true);
    setEditValue(title);
    editValueRef.current = title;
    dirtyRef.current = false;
    setEditing(true);
    inputRef.current?.focus();
    inputRef.current?.select();

    const controller = new AbortController();
    abortRef.current = controller;

    suggestTitle(threadId, controller.signal)
      .then(s => { if (!controller.signal.aborted) setSuggestion(s); })
      .catch(() => { /* silently fail — user can still type their own */ })
      .finally(() => { if (!controller.signal.aborted) setSuggesting(false); });
  }, [editing, threadId, title]);

  const save = useCallback(async (newTitle: string) => {
    if (savingRef.current) return;
    savingRef.current = true;
    try {
      const next = normalizeRename(newTitle, title, dirtyRef.current);
      if (next === null) {
        setEditing(false);
        return;
      }
      try {
        await renameThread(threadId, next);
        setEditing(false);
      } catch (err) {
        showToast(`Failed to rename thread: ${errorDetail(err)}`, 'error');
      }
    } finally {
      savingRef.current = false;
    }
  }, [threadId, title]);

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      void save(editValueRef.current);
    } else if (e.key === 'Escape') {
      // Cancel. With data-escape-self, the central Escape policy leaves this
      // input focused so this target-phase handler runs and reverts the edit.
      // editValueRef is reset to the original title too, so a blur that races
      // this (focus leaving as the editor closes) normalizes to a no-op save.
      e.preventDefault();
      dirtyRef.current = false;
      editValueRef.current = title;
      setEditing(false);
    }
  }, [save, title]);

  const acceptSuggestion = useCallback(() => {
    if (!suggestion) return;
    dirtyRef.current = true;
    void save(suggestion);
  }, [suggestion, save]);

  // Shared by the desktop <input> and mobile <textarea> edit fields. Collapse
  // any newline (a paste can introduce one into the <textarea>) to a space — a
  // title is a single logical line, which a desktop <input> enforces natively.
  const handleInput = useCallback((e: Event) => {
    const value = (e.target as HTMLInputElement | HTMLTextAreaElement).value.replace(/\r?\n/g, ' ');
    setEditValue(value);
    editValueRef.current = value;
    dirtyRef.current = true;
  }, []);

  const handleBlur = useCallback(() => void save(editValueRef.current), [save]);

  return (
    <div class={`thread-title-edit${editing ? ' is-editing' : ''}`}>
      {/* Read-only display is a div element, deliberately not a textarea: in the
          desktop header CSS gives it white-space:nowrap + align-self:stretch so
          it hugs the title on one line (a textarea would size to its cols
          attribute and wrap early), and a title too long for the row truncates
          with a CSS text-overflow ellipsis at the row's right edge. */}
      <div
        class="thread-title-input thread-title-display"
        onClick={startEditing}
      >{title}</div>
      {/* Mobile: a <textarea> so a long title wraps to multiple lines and the
          whole text stays visible while editing (autoResizeTextarea grows it to
          fit). Desktop: an <input> that hugs its value via the size attr so the
          action icons sit right beside the title on one line. Both opt out of the
          global "Esc defocuses" gesture (data-escape-self): a blur here commits
          the rename, so letting the central policy blur on Escape would SAVE
          instead of cancel — handleKeyDown's Escape branch reverts instead. */}
      {isMobile ? (
        <textarea
          ref={setInputEl}
          class="thread-title-input thread-title-edit-input"
          value={editValue}
          rows={1}
          onFocus={startEditing}
          onInput={handleInput}
          onKeyDown={handleKeyDown}
          onBlur={editing ? handleBlur : undefined}
          data-escape-self
          tabIndex={editing ? 0 : -1}
          {...PROSE_TEXT_ATTRS}
        />
      ) : (
        <input
          ref={setInputEl}
          type="text"
          class="thread-title-input thread-title-edit-input"
          value={editValue}
          // Hug the value while editing (override the base width:100% with width:auto
          // in CSS). size is universal, unlike field-sizing which isn't everywhere yet.
          size={Math.max(4, editValue.length)}
          onFocus={startEditing}
          onInput={handleInput}
          onKeyDown={handleKeyDown}
          onBlur={editing ? handleBlur : undefined}
          data-escape-self
          // The tooltip lives on this transparent overlay <input>, not on the
          // read-only display div beneath it: the overlay (position:absolute,
          // inset:0) is what the pointer actually hits while idle, and the
          // global tooltip system walks UP from the hovered node — it never
          // reaches the display div (a sibling, not an ancestor). Cleared while
          // editing so it doesn't pop over the rename field.
          data-tooltip={editing ? undefined : 'Edit thread name (can suggest new name)'}
          tabIndex={editing ? 0 : -1}
          {...PROSE_TEXT_ATTRS}
        />
      )}
      {/* Suggestion sits below the input but is taken out of flow (absolute, see
          CSS) so it doesn't make .thread-title-edit taller — otherwise the
          header's align-items:center would drop the action icons to the middle
          of input+suggestion instead of keeping them centered on the input. */}
      {editing && (
        <div class="thread-title-suggestion">
          {suggesting ? (
            showSuggesting ? <div class="thread-title-shimmer" /> : null
          ) : suggestion && suggestion !== title ? (
            <button
              class="thread-title-suggestion-btn"
              onMouseDown={(e) => { e.preventDefault(); acceptSuggestion(); }}
            >
              {suggestion}
            </button>
          ) : null}
        </div>
      )}
    </div>
  );
}
