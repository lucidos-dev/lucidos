import { useEffect, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { showToast, showConfirm, permissionGrantsVersion } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { toFailed, loadingIfFresh, type Loadable } from '../../store/types';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf } from '../shared/Skeleton';
import { AllowlistRow } from '../shared/AllowlistRow';
import { LoadingFade } from '../shared/LoadingFade';
import { Explainer } from '../shared/Explainer';

interface AllowlistEditorProps {
  title: string;
  anchor: string;
  description: ComponentChildren;
  lead?: ComponentChildren;
  rememberButton: string;
  placeholder: string;
  noun: string;
  load: () => Promise<string>;
  save: (contents: string) => Promise<void>;
}

export function parseAllowlist(contents: string): { header: string[]; patterns: string[] } {
  const lines = contents.split('\n');
  const header = lines.filter((l) => l.trim().startsWith('#'));
  const patterns = lines.map((l) => l.trim()).filter((l) => l.length > 0 && !l.startsWith('#'));
  return { header, patterns };
}

export function serializeAllowlist(header: string[], patterns: string[]): string {
  const clean = patterns.map((p) => p.trim()).filter((p) => p.length > 0);
  return [...header, ...clean].join('\n') + '\n';
}

export function mergeAllowlist(base: string[], mine: string[], theirs: string[]): string[] {
  const set = (rows: string[]) => new Set(rows.map((p) => p.trim()).filter((p) => p.length > 0));
  const inBase = set(base);
  const inMine = set(mine);
  const inTheirs = set(theirs);
  const kept = mine.filter((p) => !p.trim() || inTheirs.has(p.trim()) || !inBase.has(p.trim()));
  const landed = [...inTheirs].filter((p) => !inBase.has(p) && !inMine.has(p));
  return [...kept, ...landed];
}

export function AllowlistEditor(props: AllowlistEditorProps) {
  const [loadable, setLoadable] = useState<Loadable<string>>({ status: 'not-loaded' });
  const [header, setHeader] = useState<string[]>([]);
  const [patterns, setPatterns] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const showLoading = useDelayedLoading(loadable);
  const dirty = loadable.status === 'loaded' && serializeAllowlist(header, patterns) !== loadable.data;
  const edits = useRef(0);

  function edit(apply: () => void): void {
    edits.current++;
    apply();
  }

  function reload(): Promise<void> {
    const startedAt = edits.current;
    setLoadable(loadingIfFresh);
    return props.load()
      .then((contents) => {
        if (edits.current !== startedAt) return;
        const parsed = parseAllowlist(contents);
        setHeader(parsed.header);
        setPatterns(parsed.patterns);
        setLoadable({ status: 'loaded', data: serializeAllowlist(parsed.header, parsed.patterns) });
      })
      .catch((e) => {
        if (edits.current !== startedAt) return;
        setLoadable(toFailed(e));
      });
  }

  useEffect(() => { void reload(); }, []);
  usePanelRefresh(props.noun, dirty ? null : reload);

  useVersionedRefresh(permissionGrantsVersion.value, dirty, reload);

  function setPatternAt(i: number, value: string) {
    edit(() => setPatterns((prev) => prev.map((p, idx) => (idx === i ? value : p))));
  }

  async function deletePatternAt(i: number) {
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
      const onDisk = parseAllowlist(await props.load());
      const next = serializeAllowlist(onDisk.header, mergeAllowlist(loaded, drafted, onDisk.patterns));
      await props.save(next);
      const persisted = parseAllowlist(next);
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
        {props.lead}
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
      {props.lead}
      <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf containerClass="allowlist-rows" row={() => <AllowlistRow />} />}>
        {loadable.status === 'loaded' ? (
          <>
            <div class="allowlist-rows">
              {patterns.length === 0 && (
                <div class="allowlist-empty">No patterns yet. Use the <strong>{props.rememberButton}</strong> buttons on permission prompts, or add one below.</div>
              )}
              {patterns.map((pattern, i) => (
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
