import { Disclosure } from '../shared/Disclosure';
import type { MentionCandidate } from './mentions';

/** The mentions on offer while an `@` word is typed (ADR 0649). Tab takes the
 *  first; a tap takes any. Enter still sends. */
export function MentionSuggestions({ candidates, onPick }: {
  candidates: MentionCandidate[];
  onPick: (candidate: MentionCandidate) => void;
}) {
  return (
    <Disclosure open={candidates.length > 0}>
      <div class="mention-suggestions" data-role="mention-suggestions" role="group" aria-label="Mention someone">
        {candidates.map((c) => (
          <button
            key={c.token}
            type="button"
            class="mention-suggestion"
            // Keeps the caret in the composer, where the pick lands.
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onPick(c)}
          >
            <span class="mention-suggestion-label">{c.label}</span>
            {c.hint && <span class="mention-suggestion-hint">{c.hint}</span>}
          </button>
        ))}
      </div>
    </Disclosure>
  );
}
