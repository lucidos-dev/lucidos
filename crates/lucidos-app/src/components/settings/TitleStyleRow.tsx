import { useEffect } from 'preact/hooks';
import { showToast } from '../../store/store';
import { savePreference, storedTitleStyle } from '../../store/actions/preferences';
import { deletePreference } from '../../api/client';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { errorDetail } from '../../utils/errorDetail';
import { useAutoResizeTextarea } from '../../hooks/useAutoResizeTextarea';
import { isImeComposingKey } from '../../utils/ime';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { Explainer } from '../shared/Explainer';
import { PREF_TITLE_STYLE } from '@lucidos/preference-catalog';
import { titleStyleCommit, titleStyleOneLine } from './titleStyle';

/** Settings → Models → Background tasks → Title generation → Title style: one
 *  instruction the title model follows when it names a thread. Commits on blur
 *  or Enter. Reset deletes the row, which restores the standard titles. */
export function TitleStyleRow() {
  const stored = storedTitleStyle();
  const [draft, setDraft] = useServerBackedField(stored);
  const maxChars = PREF_TITLE_STYLE.maxChars;
  const field = useAutoResizeTextarea(draft);
  // Re-arm once a save, a Reset or another device lands the draft's own text.
  // The row stays mounted, so without it the field ignores every later frame.
  useEffect(() => {
    if (draft.trim() === stored.trim()) setDraft(stored);
  }, [stored, draft]);

  async function clear() {
    try {
      const result = await deletePreference(PREF_TITLE_STYLE.key);
      if (result.success) setDraft('');
      else showToast(result.error || 'Failed to reset the title style', 'error');
    } catch (e) {
      showToast(`Failed to reset the title style: ${errorDetail(e)}`, 'error');
    }
  }

  async function commit() {
    const action = titleStyleCommit(draft, stored, maxChars);
    if (action.kind === 'clear') await clear();
    else if (action.kind === 'save') await savePreference(PREF_TITLE_STYLE.key, action.value);
    else if (action.kind === 'too-long') showToast(`A title style is at most ${maxChars} characters`, 'error');
  }

  return (
    <div class="settings-row settings-row-child" data-search-anchor="models:title-style">
      <span class="settings-row-label">
        Title style
        <Explainer title="Title style">
          <p>
            How the title model words a new thread's title, in plain language: for example{' '}
            <code>lower-kebab-case, at most 4 words</code>. It overrides the standard title
            length and wording.
          </p>
          <p>
            A title always stays short. It applies to titles written from now on, and existing
            threads keep their names. Reset restores the standard titles.
          </p>
        </Explainer>
      </span>
      <textarea
        ref={field}
        class="settings-text-input title-style-input"
        aria-label="Title style"
        placeholder={`Standard: ${PREF_TITLE_STYLE.standard}`}
        rows={1}
        value={draft}
        onInput={(e) => setDraft(titleStyleOneLine((e.target as HTMLTextAreaElement).value))}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !isImeComposingKey(e)) {
            e.preventDefault();
            (e.target as HTMLTextAreaElement).blur();
          }
        }}
        {...PROSE_TEXT_ATTRS}
      />
      <div class="title-style-foot">
        <span class="list-row-details">{[...draft].length}/{maxChars}</span>
        {stored.trim() !== '' && (
          <button type="button" class="action-btn action-btn-secondary" onClick={() => void clear()}>
            Reset
          </button>
        )}
      </div>
    </div>
  );
}
