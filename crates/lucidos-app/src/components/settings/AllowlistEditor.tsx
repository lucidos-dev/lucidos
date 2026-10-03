import { useEffect, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { showToast, showConfirm, permissionGrantsVersion } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { toFailed, loadingIfFresh, type Loadable } from '../../store/types';
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
  /** Load the raw allowlist file contents. Save re-reads through it, to merge. */
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

/** Three-way merge of a draft onto the file as it is now. `base` is what the
 *  draft started from, `mine` the draft, `theirs` the current file.
 *
 *  A pattern the draft added or kept stays, unless the file dropped a kept one.
 *  A pattern the draft deleted stays deleted. The merge appends a pattern that
 *  appeared in the file since `base`, so a grant made meanwhile survives. Patterns
 *  compare trimmed, and `mine`'s rows keep their raw text, empty rows included,
 *  so the merge never rewrites a row the user is typing in. */
export function mergeAllowlist(base: string[], mine: string[], theirs: string[]): string[] {
  const set = (rows: string[]) => new Set(rows.map((p) => p.trim()).filter((p) => p.length > 0));
  const inBase = set(base);
  const inMine = set(mine);
  const inTheirs = set(theirs);
  const kept = mine.filter((p) => !p.trim() || inTheirs.has(p.trim()) || !inBase.has(p.trim()));
  const landed = [...inTheirs].filter((p) => !inBase.has(p) && !inMine.has(p));
  return [...kept, ...landed];
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
  const dirty = loadable.status === 'loaded' && serializeAllowlist(header, patterns) !== loadable.data;
  /** Bumped by every local write of the rows. `dirty` cannot stand in for it:
   *  `dirty` gates whether a reload STARTS, and it is read at render time,
   *  while a reply lands between renders. So a reload begun on a clean editor
   *  would still overwrite a draft typed while it was in flight. */
  const edits = useRef(0);

  /** Wrap a local write of the rows, so a reload in flight drops its reply. */
  function edit(apply: () => void): void {
    edits.current++;
    apply();
  }

  function reload(): Promise<void> {
    const startedAt = edits.current;
    // Keep the rows through the round-trip, so only a first read shows a
    // loader and an SSE-driven re-read swaps in place.
    setLoadable(loadingIfFresh);
    return props.load()
      .then((contents) => {
        // A draft appeared under this read, so the file it holds is older than
        // what is on screen. Dropping the error below is right for the same
        // reason: the rows the user is editing are still there, and their Save
        // reports its own result.
        if (edits.current !== startedAt) return;
        const parsed = parseAllowlist(contents);
        setHeader(parsed.header);
        setPatterns(parsed.patterns);
        // Store the NORMALIZED baseline (not the raw file) so `dirty` compares
        // like-for-like. Otherwise a file with blank lines / interspersed
        // comments / a 0-byte body would read as dirty before any edit, because
        // serialize() canonicalizes whitespace and comment placement.
        setLoadable({ status: 'loaded', data: serializeAllowlist(parsed.header, parsed.patterns) });
      })
      .catch((e) => {
        if (edits.current !== startedAt) return;
        setLoadable(toFailed(e));
      });
  }

  // props.load is a stable API-client function; load once on mount.
  useEffect(() => { void reload(); }, []);
  // Paused while dirty, for the reason the SSE re-read below is.
  usePanelRefresh(props.noun, dirty ? null : reload);

  // The agent grants a permission by writing this very file, so the editor has
  // to follow it (ADR 0118). Paused while dirty: unsaved patterns are the
  // user's and a re-read would drop them. Save merges what landed meanwhile.
  // Save and Revert both clear dirty, which is when a held-back frame lands.
  useVersionedRefresh(permissionGrantsVersion.value, dirty, reload);

  function setPatternAt(i: number, value: string) {
    edit(() => setPatterns((prev) => prev.map((p, idx) => (idx === i ? value : p))));
  }

  async function deletePatternAt(i: number) {
    // An unfilled "Add pattern" row holds no permission — drop it silently.
    // For a real pattern, removing it revokes a granted permission, so confirm.
    const pattern = patterns[i]?.trim() ?? '';
    if (pattern && !(await showConfirm(`Delete permission "${pattern}"?`, 'Delete', { variant: 'danger' }))) {
      return;
    }
    edit(() => setPatterns((prev) => prev.filter((_, idx) => idx !== i)));
  }

  async function save() {
    if (loadable.status !== 'loaded') return;
    const loaded = parseAllowlist(loadable.data).patterns;
    const drafted = patterns;
    const startedAt = edits.current;
    setSaving(true);
    try {
      // The file may have moved under the draft: an "Always allow" appends to
      // it while the re-read is paused. Writing the draft alone would revoke
      // that grant, so Save does not write a file it cannot re-read.
      const onDisk = parseAllowlist(await props.load());
      const next = serializeAllowlist(onDisk.header, mergeAllowlist(loaded, drafted, onDisk.patterns));
      await props.save(next);
      const persisted = parseAllowlist(next);
      // The rows stay editable during the save. Rows edited meanwhile are a
      // newer draft, so they stay, with what the merge brought in. Otherwise
      // the rows collapse to their persisted form.
      const typedMeanwhile = edits.current !== startedAt;
      edit(() => {
        setHeader(persisted.header);
        setPatterns((typed) => (typedMeanwhile ? mergeAllowlist(drafted, typed, persisted.patterns) : persisted.patterns));
      });
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
    edit(() => {
      setHeader(parsed.header);
      setPatterns(parsed.patterns);
    });
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
                onClick={() => edit(() => setPatterns((prev) => [...prev, '']))}
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
