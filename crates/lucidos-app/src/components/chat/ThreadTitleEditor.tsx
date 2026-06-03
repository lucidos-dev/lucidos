import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { renameThread, suggestTitle } from '../../api/threads';
import { showToast } from '../../store/store';
import { autoResizeTextarea } from '../../utils/dom';
import { errorDetail } from '../../utils/errorDetail';

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
  const inputRef = useRef<HTMLInputElement>(null);
  const displayRef = useRef<HTMLTextAreaElement>(null);
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

  // Deps are [title, editing] — autoresizing on editValue would churn the iOS
  // textarea layout per keystroke and clobber cursor/selection. The editing
  // gate skips the call while the display textarea is display:none (its
  // scrollHeight reads 0 → height collapses to ~2px and stays there once the
  // editor closes). Re-running on the editing→false transition refits height
  // to the latest title delivered by SSE during the save.
  useEffect(() => {
    if (!editing) autoResizeTextarea(displayRef.current);
  }, [title, editing]);

  // Container width changes (drawer toggle, divider drag, window resize) don't
  // fire the [title, editing] effect — without this observer, scrollHeight set
  // at a narrow width (where the title wrapped to multiple lines) stays pinned
  // as inline style.height after the container widens, ballooning the header
  // until rename or reload. Width-only guard ignores the height churn from our
  // own style.height writes, which would otherwise cause a feedback loop.
  useEffect(() => {
    const el = displayRef.current;
    if (!el || editing) return;
    let lastWidth = el.clientWidth;
    const observer = new ResizeObserver(() => {
      const newWidth = el.clientWidth;
      if (newWidth === lastWidth) return;
      lastWidth = newWidth;
      autoResizeTextarea(el);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [editing]);

  // The overlay input keeps DOM focus after save/escape, so onFocus won't
  // re-fire on the next click — blur explicitly so editing can be re-entered
  // without clicking elsewhere first.
  useEffect(() => {
    if (!editing) inputRef.current?.blur();
  }, [editing]);

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

  return (
    <div class={`thread-title-edit${editing ? ' is-editing' : ''}`}>
      <textarea
        ref={displayRef}
        class="thread-title-input thread-title-display"
        rows={1}
        value={title}
        readOnly
        tabIndex={-1}
        onClick={startEditing}
        data-tooltip="Click to rename"
      />
      <input
        ref={inputRef}
        type="text"
        class="thread-title-input thread-title-edit-input"
        value={editValue}
        onFocus={startEditing}
        onInput={(e) => {
          const value = (e.target as HTMLInputElement).value;
          setEditValue(value);
          editValueRef.current = value;
          dirtyRef.current = true;
        }}
        onKeyDown={handleKeyDown}
        onBlur={editing ? () => void save(editValueRef.current) : undefined}
        // Opt out of the global "Esc defocuses" gesture (dispatchEscape): a
        // blur here commits the rename, so letting the central policy blur on
        // Escape would SAVE instead of cancel. With this marker, focus stays
        // put and handleKeyDown's Escape branch reverts the edit.
        data-escape-self
        tabIndex={editing ? 0 : -1}
      />
      {editing && (
        <div class="thread-title-suggestion">
          {suggesting ? (
            <div class="thread-title-shimmer" />
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
