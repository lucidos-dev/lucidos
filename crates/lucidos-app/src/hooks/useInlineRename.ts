import { useEffect, useRef, useState } from 'preact/hooks';

export function useInlineRename(served: string, onRename: (next: string) => Promise<unknown>) {
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(served);
  const renamedRef = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { if (!renaming) inputRef.current?.blur(); }, [renaming]);

  useEffect(() => { if (!renaming) setDraft(served); }, [served, renaming]);

  async function commit(): Promise<void> {
    if (renamedRef.current) return;
    const trimmed = draft.trim();
    if (!trimmed || trimmed === served) { setRenaming(false); return; }
    renamedRef.current = true;
    try {
      await onRename(trimmed);
    } finally {
      setRenaming(false);
    }
  }

  function open(): void {
    renamedRef.current = false;
    setDraft(served);
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
