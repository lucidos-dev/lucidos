import { useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { showToast, showConfirm } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { toFailed, type Loadable } from '../../store/types';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Explainer } from '../shared/Explainer';

interface AllowlistEditorProps {
  /** Section heading (a `settings-section-title`). */
  title: string;
  /** `data-search-anchor` for the section title, so Search Everywhere can land here. */
  anchor: string;
  /** What this allowlist is for, behind the heading's *explainer*. On the
   *  heading rather than inside the loaded branch, so it stays readable while
   *  the file is loading and after a failed load. */
  description: ComponentChildren;
  /** Placeholder shown in an empty pattern row. */
  placeholder: string;
  /** Noun used in the failed-load message ("tool permissions" / "command permissions"). */
  noun: string;
  /** Load the raw allowlist file contents. */
  load: () => Promise<string>;
  /** Persist the raw allowlist file contents (whole-file overwrite). */
  save: (contents: string) => Promise<void>;
}

/** Split allowlist file contents into the leading `#` comment header (preserved
 *  verbatim, never shown as an editable row) and the editable pattern lines
 *  (trimmed; blanks and comments dropped). */
export function parseAllowlist(contents: string): { header: string[]; patterns: string[] } {
  const lines = contents.split('\n');
  const header = lines.filter((l) => l.trim().startsWith('#'));
  const patterns = lines.map((l) => l.trim()).filter((l) => l.length > 0 && !l.startsWith('#'));
  return { header, patterns };
}

/** Reassemble header + patterns into file contents. Patterns are trimmed and
 *  empties dropped, so an unfilled "Add" row never persists as a blank line. */
export function serializeAllowlist(header: string[], patterns: string[]): string {
  const clean = patterns.map((p) => p.trim()).filter((p) => p.length > 0);
  return [...header, ...clean].join('\n') + '\n';
}

/** Self-skeletonizing pattern row: rendered with no props inside a
 *  SkeletonProvider (`<AllowlistRow />`) it draws itself as a loading placeholder
 *  (an input-sized block + a small delete block) via the Sk* leaves; with real
 *  props it renders the editable input + Delete button. The pattern list +
 *  per-row callbacks live in the parent, so the skeleton call passes nothing. */
function AllowlistRow({ pattern, placeholder, onInput, onDelete }: {
  pattern?: string;
  placeholder?: string;
  onInput?: (value: string) => void;
  onDelete?: () => void;
}) {
  return (
    <div class="allowlist-row">
      <SkBlock w="100%" h="2.25rem" round>
        {/* autocorrect/autocapitalize are deliberately NOT declared here: the
            global stamp (utils/noAutofill.ts) already turns both off for every
            field, and declaring autocorrect="off" in JSX would actually turn it
            ON — Preact assigns it as a DOM property, and `autocorrect`'s IDL
            attribute is a boolean, so the non-empty string "off" coerces to
            true. The stamp's setAttribute writes the literal keyword instead. */}
        <input
          class="allowlist-row-input"
          type="text"
          spellcheck={false}
          autocomplete="off"
          placeholder={placeholder}
          value={pattern}
          onInput={(e) => onInput?.((e.target as HTMLInputElement).value)}
        />
      </SkBlock>
      <SkBlock w="3.75rem" h="2.25rem" round>
        <button
          type="button"
          class="action-btn action-btn-danger"
          aria-label={`Delete pattern ${pattern || '(empty)'}`}
          onClick={() => onDelete?.()}
        >
          Delete
        </button>
      </SkBlock>
    </div>
  );
}

/** A list editor over a one-pattern-per-line allowlist file (`cc-allowed-tools`
 *  or `agent-allowed-commands`). Each pattern is an editable row with a delete
 *  button; "Add pattern" appends a row; Save/Revert persist the whole file. The
 *  file's `#` header comment is preserved across edits. */
export function AllowlistEditor(props: AllowlistEditorProps) {
  const [loadable, setLoadable] = useState<Loadable<string>>({ status: 'not-loaded' });
  const [header, setHeader] = useState<string[]>([]);
  const [patterns, setPatterns] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    setLoadable({ status: 'loading' });
    props.load()
      .then((contents) => {
        const parsed = parseAllowlist(contents);
        setHeader(parsed.header);
        setPatterns(parsed.patterns);
        // Store the NORMALIZED baseline (not the raw file) so `dirty` compares
        // like-for-like. Otherwise a file with blank lines / interspersed
        // comments / a 0-byte body would read as dirty before any edit, because
        // serialize() canonicalizes whitespace and comment placement.
        setLoadable({ status: 'loaded', data: serializeAllowlist(parsed.header, parsed.patterns) });
      })
      .catch((e) => setLoadable(toFailed(e)));
    // props.load is a stable API-client function; load once on mount.
  }, []);

  function setPatternAt(i: number, value: string) {
    setPatterns((prev) => prev.map((p, idx) => (idx === i ? value : p)));
  }

  async function deletePatternAt(i: number) {
    // An unfilled "Add pattern" row holds no permission — drop it silently.
    // For a real pattern, removing it revokes a granted permission, so confirm.
    const pattern = patterns[i]?.trim() ?? '';
    if (pattern && !(await showConfirm(`Delete permission "${pattern}"?`, 'Delete', { variant: 'danger' }))) {
      return;
    }
    setPatterns((prev) => prev.filter((_, idx) => idx !== i));
  }

  async function save() {
    const next = serializeAllowlist(header, patterns);
    setSaving(true);
    try {
      await props.save(next);
      // Re-parse so trimmed/empty rows collapse to their persisted form.
      const parsed = parseAllowlist(next);
      setHeader(parsed.header);
      setPatterns(parsed.patterns);
      setLoadable({ status: 'loaded', data: next });
      showToast('Saved', 'info');
    } catch (e) {
      showToast(`Save failed: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  }

  function revert() {
    if (loadable.status !== 'loaded') return;
    const parsed = parseAllowlist(loadable.data);
    setHeader(parsed.header);
    setPatterns(parsed.patterns);
  }

  if (loadable.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor={props.anchor}>
          {props.title}
          <Explainer title={props.title}>{props.description}</Explainer>
        </div>
        <LoadableError noun={props.noun} error={loadable.error} />
      </div>
    );
  }

  const dirty = loadable.status === 'loaded' && serializeAllowlist(header, patterns) !== loadable.data;

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor={props.anchor}>
        {props.title}
        <Explainer title={props.title}>{props.description}</Explainer>
      </div>
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf containerClass="allowlist-rows" row={() => <AllowlistRow />} />}>
        {loadable.status === 'loaded' ? (
          <>
            <div class="allowlist-rows">
              {patterns.length === 0 && (
                <div class="allowlist-empty">No patterns yet. Use the <strong>Always allow</strong> buttons on permission prompts, or add one below.</div>
              )}
              {patterns.map((pattern, i) => (
                // Positional key: rows have no stable id and edits mutate in place
                // rather than reorder, so the index is a correct identity here.
                <AllowlistRow
                  key={i}
                  pattern={pattern}
                  placeholder={props.placeholder}
                  onInput={(value) => setPatternAt(i, value)}
                  onDelete={() => void deletePatternAt(i)}
                />
              ))}
            </div>
            <div class="allowlist-actions">
              <button
                type="button"
                class="action-btn"
                onClick={() => setPatterns((prev) => [...prev, ''])}
              >
                Add pattern
              </button>
              <span class="allowlist-actions-spacer" />
              <button
                type="button"
                class="action-btn"
                disabled={!dirty || saving}
                onClick={revert}
              >
                Revert
              </button>
              <button
                type="button"
                class="action-btn action-btn-confirm"
                disabled={!dirty || saving}
                onClick={save}
              >
                {saving ? 'Saving...' : 'Save'}
              </button>
            </div>
          </>
        ) : null}
      </LoadingFade>
    </div>
  );
}
