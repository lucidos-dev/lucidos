import { useEffect, useRef, useState } from 'preact/hooks';

/** The state behind a rename-in-place field: a name a surface shows, a control
 *  that opens it for editing, and Enter / blur / Escape.
 *
 *  Each piece below is one a hand-rolled copy gets wrong in its own way, so
 *  they live here rather than beside the markup. The field is MOUNTED whether
 *  or not it is open, because `open` has to focus it inside the user's tap.
 *
 *  `onRename` is the caller's own action, so the toast and the error path stay
 *  with the entity. It should report its own failure and resolve, as both
 *  callers do. One that rejects still closes the field and re-seeds the draft,
 *  and the rejection surfaces rather than being swallowed here. */
export function useInlineRename(served: string, onRename: (next: string) => Promise<unknown>) {
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(served);
  // Enter and blur both commit, and closing the field fires a trailing blur
  // that would rename a second time. Reset when the field reopens.
  const renamedRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  // Enter commits without blurring, and the field stays mounted. Closing would
  // otherwise hold the mobile keyboard up over a surface with nothing to type
  // into.
  useEffect(() => { if (!renaming) inputRef.current?.blur(); }, [renaming]);

  // Idle, the draft mirrors the served value, so a rename from another device
  // repaints it. Seeded once at mount it would offer the stale name back on the
  // next tap (ADR 0118). It also resets on every close, so an abandoned edit
  // and a failed one both settle on the stored name.
  useEffect(() => { if (!renaming) setDraft(served); }, [served, renaming]);

  async function commit(): Promise<void> {
    if (renamedRef.current) return;
    const trimmed = draft.trim();
    if (!trimmed || trimmed === served) { setRenaming(false); return; }
    renamedRef.current = true;
    // `finally`, because the guard above is already latched. A rejected rename
    // that left the field open would refuse every later Enter and blur, and
    // the user's text with it.
    try {
      await onRename(trimmed);
    } finally {
      setRenaming(false);
    }
  }

  function open(): void {
    renamedRef.current = false;
    setDraft(served);
    // Synchronous, and before the state flip: iOS raises the keyboard only for
    // a focus() inside the tap, and Preact renders afterwards in a microtask
    // that no longer carries one.
    inputRef.current?.focus();
    inputRef.current?.select();
    setRenaming(true);
  }

  return {
    renaming,
    draft,
    setDraft,
    inputRef,
    open,
    commit,
    cancel: () => setRenaming(false),
  };
}
