import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import { appSearchOpen, appSearchQuery } from '../../store/store';
import { closeAppSearch } from '../../store/actions/apps';
import { CloseIcon } from '../shared/icons';
import { Disclosure } from '../shared/Disclosure';
import { SearchField } from '../shared/SearchField';

interface Props {
  placeholder: string;
  dataRole: string;
}

function SearchRow({ placeholder, dataRole, inputRef }: Props & { inputRef: RefObject<HTMLInputElement> }) {
  return (
    <div class="apps-search-bar">
      <SearchField
        class="apps-search-field"
        inputRef={inputRef}
        data-role={dataRole}
        placeholder={placeholder}
        value={appSearchQuery.value}
        onInput={(e) => { appSearchQuery.value = e.currentTarget.value; }}
        onKeyDown={(e) => { if (e.key === 'Escape') closeAppSearch(); }}
      />
      <button class="icon-btn header-icon" onClick={closeAppSearch} aria-label="Close search">
        <CloseIcon />
      </button>
    </div>
  );
}

/** The search row the header's search toggle opens over the Apps and Plugins
 *  lists. It rolls in and out with the list below it. It focuses on every
 *  open, not on mount: a reopen during the exit roll finds the row mounted. */
export function AppSearchBar(props: Props) {
  const open = appSearchOpen.value;
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) inputRef.current?.focus({ preventScroll: true });
  }, [open]);
  return (
    <Disclosure open={open}>
      <SearchRow {...props} inputRef={inputRef} />
    </Disclosure>
  );
}
