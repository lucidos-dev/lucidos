import { useSignal } from '@preact/signals';
import { useRef } from 'preact/hooks';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import { isTextInput } from '../../utils/dom';
import { hasCoarsePointer, isMobile } from '../../utils/viewport';
import { isTypeaheadKey } from './typeahead';

export { filterListItems, listFilterVisible } from '../../utils/listFilter';
import { listFilterVisible } from '../../utils/listFilter';

/** Shared query and reveal behavior extracted from Dropdown. Focus and row
 *  selection stay with the host, including its keyboard handoff. */
export function useListFilter({ enabled = true, escape = true, onChange }: {
  enabled?: boolean;
  escape?: boolean;
  onChange?: (query: string) => void;
} = {}) {
  const query = useSignal('');
  const inputRef = useRef<HTMLInputElement>(null);
  const searching = query.value.length > 0;
  const visible = listFilterVisible({ query: query.value, coarse: hasCoarsePointer(), narrow: isMobile() });

  function setQuery(value: string) {
    query.value = value;
    onChange?.(value);
  }
  const clear = () => setQuery('');
  useEscapeStep(enabled && escape && searching ? clear : null);

  function handleKeyDown(e: KeyboardEvent): boolean {
    if (!enabled || e.isComposing || isTextInput(e.target)) return false;
    const unmodified = !e.metaKey && !e.ctrlKey && !e.altKey;
    const deleting = e.key === 'Backspace' && unmodified && query.value.length > 0;
    if (!deleting && !isTypeaheadKey(e) && !(e.key === ' ' && unmodified)) return false;
    e.preventDefault();
    e.stopPropagation();
    setQuery(deleting ? Array.from(query.value).slice(0, -1).join('') : query.value + e.key);
    return true;
  }

  return { query, inputRef, searching, visible, setQuery, clear, handleKeyDown };
}
