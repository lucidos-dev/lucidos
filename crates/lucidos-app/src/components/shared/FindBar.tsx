import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';
import {
  closeFind, findFocusRequest, findQuery, findResult, findStatusText, findSurface,
  setFindQuery, stepFind, takeFindRequest, type FindSurface,
} from '../../store/actions/find-bar';
import { findRequest } from '../../store/store';
import { pushOverlay, removeOverlay } from '../../store/overlayStack';
import { ChevronDownIcon, ChevronUpIcon, CloseIcon } from './icons';
import { Disclosure } from './Disclosure';
import { SearchField } from './SearchField';

function FindRow({ inputRef, placeholder }: { inputRef: RefObject<HTMLInputElement>; placeholder: string }) {
  const result = findResult.value;
  const canStep = result.status === 'found' && result.total > 0;
  return (
    <div class="find-bar" data-role="find-bar">
      <SearchField
        class="find-field"
        inputRef={inputRef}
        data-role="find-input"
        placeholder={placeholder}
        value={findQuery.value}
        onInput={(e) => setFindQuery(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key !== 'Enter') return;
          e.preventDefault();
          void stepFind(e.shiftKey ? -1 : 1);
        }}
      >
        <span
          class={`find-status${result.status === 'unsearchable' ? ' is-failed' : ''}`}
          data-role="find-status"
          aria-live="polite"
        >
          {findStatusText(result)}
        </span>
      </SearchField>
      <button class="icon-btn header-icon" onClick={() => void stepFind(-1)} disabled={!canStep} aria-label="Previous match">
        <ChevronUpIcon />
      </button>
      <button class="icon-btn header-icon" onClick={() => void stepFind(1)} disabled={!canStep} aria-label="Next match">
        <ChevronDownIcon />
      </button>
      <button class="icon-btn header-icon" onClick={closeFind} aria-label="Close find">
        <CloseIcon />
      </button>
    </div>
  );
}

/** The find bar on one surface. It rolls in above what it searches. Escape
 *  closes it from the field and from inside a frame, through the overlay
 *  stack. A new `scope` (another app, file or thread) ends the session, since
 *  its text and highlights belonged to the old one. */
export function FindBar({ surface, scope, placeholder }: {
  surface: FindSurface;
  scope: string;
  placeholder: string;
}) {
  const open = findSurface.value === surface;
  const focusRequest = findFocusRequest.value;
  const request = findRequest.value;
  const inputRef = useRef<HTMLInputElement>(null);

  // Keyed on the request alone: an open that asks for no focus leaves the caret.
  useEffect(() => {
    if (!open) return;
    inputRef.current?.focus({ preventScroll: true });
    inputRef.current?.select();
  }, [focusRequest]);

  // One entry per surface: a hand-off from one bar to the other removes the
  // closing bar's entry and must not take the opening one with it.
  useEffect(() => {
    if (!open) return;
    const id = `find-bar:${surface}`;
    pushOverlay({ id, dismiss: closeFind, hasPanel: false });
    return () => removeOverlay(id);
  }, [open, surface]);

  // Unmount and a scope change both end this surface's session, and take its
  // highlights with it.
  useEffect(() => () => {
    if (findSurface.value === surface) closeFind();
  }, [surface, scope]);

  // After the cleanup above, so a request opens on a session of its own.
  useEffect(() => {
    if (request) takeFindRequest(surface, scope);
  }, [request, surface, scope]);

  return (
    <Disclosure open={open} class="find-bar-slot">
      <FindRow inputRef={inputRef} placeholder={placeholder} />
    </Disclosure>
  );
}
