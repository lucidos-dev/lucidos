import { Disclosure } from '../shared/Disclosure';
import type { MentionCandidate } from './mentions';

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
