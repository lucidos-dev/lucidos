import { useState, useRef, useEffect, useCallback } from 'preact/hooks';
import { renameThread, suggestTitle } from '../../api/threads';
import { showToast } from '../../store/store';
import { autoResizeTextarea } from '../../utils/dom';

interface Props {
  threadId: string;
  title: string;
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
      const trimmed = newTitle.trim();
      if (!trimmed || trimmed === title) {
        setEditing(false);
        return;
      }
      try {
        await renameThread(threadId, trimmed);
        setEditing(false);
      } catch {
        showToast('Failed to rename thread', 'error');
      }
    } finally {
      savingRef.current = false;
    }
  }, [threadId, title]);

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      save(editValueRef.current);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      // Reset the ref so the blur-fired save becomes a no-op (trimmed === title).
      editValueRef.current = title;
      setEditing(false);
    }
  }, [save, title]);

  const acceptSuggestion = useCallback(() => {
    if (suggestion) save(suggestion);
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
        }}
        onKeyDown={handleKeyDown}
        onBlur={editing ? () => save(editValueRef.current) : undefined}
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
