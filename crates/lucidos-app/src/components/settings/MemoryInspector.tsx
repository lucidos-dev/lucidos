import { useState, useEffect, useCallback, useRef } from 'preact/hooks';
import { signal } from '@preact/signals';
import { getMemoryStats, getMemoryEntries, getMemorySource, rebuildMemory, cancelMemoryRebuild } from '../../api/client';
import { showToast, memoryRebuildProgress, memoryEntriesVersion } from '../../store/store';
import { useDelayedFlag, useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock, SkeletonProvider } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Disclosure } from '../shared/Disclosure';
import { setLoadingIfFresh, toFailed } from '../../store/types';
import { formatTimeAgo, formatDateTime } from '../../utils/formatTime';
import { errorDetail } from '../../utils/errorDetail';
import { copyToClipboard } from '../../utils/clipboard';
import type { Loadable } from '../../store/types';
import type {
  MemoryStatsResponse,
  MemoryEntryInfo,
  MemorySourceResponse,
} from '../../api/types';

const PAGE_SIZE = 50;

const statsLoadable = signal<Loadable<MemoryStatsResponse>>({ status: 'not-loaded' });
const entriesLoadable = signal<Loadable<{ entries: MemoryEntryInfo[]; total: number; has_more: boolean }>>({ status: 'not-loaded' });

async function loadStats() {
  setLoadingIfFresh(statsLoadable);
  try {
    const data = await getMemoryStats();
    statsLoadable.value = { status: 'loaded', data };
  } catch (e) {
    statsLoadable.value = toFailed(e);
  }
}

type ImportanceLevel = 'low' | 'medium' | 'high' | 'critical';

function ImportanceBar({ distribution, selected, onToggle }: {
  distribution: MemoryStatsResponse['importance_distribution'];
  selected: Set<ImportanceLevel>;
  onToggle: (level: ImportanceLevel) => void;
}) {
  const total = distribution.low + distribution.medium + distribution.high + distribution.critical;
  if (total === 0) return null;

  const hasFilter = selected.size > 0;
  const pct = (n: number) => `${((n / total) * 100).toFixed(1)}%`;
  const segments: { level: ImportanceLevel; count: number; label: string }[] = [
    { level: 'low', count: distribution.low, label: `Low (0\u20130.3): ${distribution.low}` },
    { level: 'medium', count: distribution.medium, label: `Medium (0.3\u20130.6): ${distribution.medium}` },
    { level: 'high', count: distribution.high, label: `High (0.6\u20130.8): ${distribution.high}` },
    { level: 'critical', count: distribution.critical, label: `Critical (0.8\u20131.0): ${distribution.critical}` },
  ];

  return (
    <div class="memory-importance-bar">
      {segments.map(({ level, count, label }) => count > 0 && (
        <div
          key={level}
          class={`memory-importance-segment ${level}${hasFilter && !selected.has(level) ? ' dimmed' : ''}${selected.has(level) ? ' selected' : ''}`}
          style={{ width: pct(count) }}
          data-tooltip={label}
          onClick={() => onToggle(level)}
        />
      ))}
    </div>
  );
}

/** The stats strip. Its labels are known up front; with no `stats`, inside a
 *  `SkeletonProvider`, its figures and importance bar shimmer. */
function MemoryStatsBar({ stats, importanceFilter = new Set(), onToggle = () => {} }: {
  stats?: MemoryStatsResponse;
  importanceFilter?: Set<ImportanceLevel>;
  onToggle?: (level: ImportanceLevel) => void;
}) {
  const d = stats?.importance_distribution;
  const hasFilter = importanceFilter.size > 0 && importanceFilter.size < 4;
  const filteredTotal = stats && d && (hasFilter
    ? (importanceFilter.has('low') ? d.low : 0)
      + (importanceFilter.has('medium') ? d.medium : 0)
      + (importanceFilter.has('high') ? d.high : 0)
      + (importanceFilter.has('critical') ? d.critical : 0)
    : stats.total);
  return (
    <div class="memory-stats-bar">
      <div class="memory-stat">
        <SkText class="memory-stat-value" w="3rem">{filteredTotal?.toLocaleString()}</SkText>
        <span class="memory-stat-label">{hasFilter ? 'Filtered' : 'Total'}</span>
      </div>
      <div class="memory-stat">
        <SkText class="memory-stat-value" w="3rem">{stats?.event_count.toLocaleString()}</SkText>
        <span class="memory-stat-label">Events</span>
      </div>
      <div class="memory-stat">
        <SkText class="memory-stat-value" w="3rem">{stats?.artifact_count.toLocaleString()}</SkText>
        <span class="memory-stat-label">Artifacts</span>
      </div>
      {d
        ? <ImportanceBar distribution={d} selected={importanceFilter} onToggle={onToggle} />
        : <div class="memory-importance-bar"><SkBlock w="100%" h="100%" /></div>}
    </div>
  );
}

/** Line widths a source block shimmers with while it loads. */
const SOURCE_SKELETON_LINES = ['70%', '88%', '55%', '80%'];

/** A memory's source text. Under a `SkeletonProvider` it draws its own box
 *  with shimmer lines. */
function SourceBlock({ text }: { text?: string }) {
  if (useSkeleton()) {
    return (
      <pre class="memory-source-pre" aria-hidden="true">
        {SOURCE_SKELETON_LINES.map((w) => <SkText key={w} as="div" w={w} />)}
      </pre>
    );
  }
  return <pre class="memory-source-pre">{text}</pre>;
}

function importanceDotClass(importance: number): string {
  if (importance >= 0.8) return 'critical';
  if (importance >= 0.6) return 'high';
  if (importance >= 0.3) return 'medium';
  return 'low';
}

/** Self-skeletonizing memory row: rendered with no entry inside a
 *  SkeletonProvider (`<MemoryEntryRow />`) it draws itself as a collapsed loading
 *  placeholder via the Sk* leaves; with a real `entry` it renders normally. The
 *  prop is optional only to support the skeleton call; the real call site passes
 *  it. The expand state stays collapsed in skeleton mode (initial state + the
 *  click handler is gated on `!sk`), so the detail section never shows. */
function MemoryEntryRow({ entry }: { entry?: MemoryEntryInfo }) {
  const sk = useSkeleton();
  const [expanded, setExpanded] = useState(false);
  const [sourceData, setSourceData] = useState<Loadable<MemorySourceResponse>>({ status: 'not-loaded' });
  const [sourceVisible, setSourceVisible] = useState(false);
  const showSourceLoading = useDelayedLoading(sourceData);

  async function toggleSource() {
    if (sourceData.status === 'loaded' || sourceData.status === 'loading') {
      setSourceVisible(!sourceVisible);
      return;
    }
    if (!entry) return;
    setSourceData({ status: 'loading' });
    setSourceVisible(true);
    try {
      const data = await getMemorySource({
        source_type: entry.source.type,
        source_id: entry.source.id,
        path: entry.source.path,
        commit: entry.source.commit,
      });
      setSourceData({ status: 'loaded', data });
    } catch (e) {
      setSourceData(toFailed(e));
    }
  }

  return (
    <div class={`list-row memory-entry-row ${expanded ? 'expanded' : ''}`} onClick={sk ? undefined : () => setExpanded(!expanded)}>
      <div class="list-row-info" style={{ cursor: 'pointer' }}>
        <div class="title list-row-name memory-entry-summary">
          <SkBlock w="4rem" h="1rem" round>
            <span class="memory-topic-badge">{entry?.topic}</span>
          </SkBlock>
          <SkText class="memory-summary-text" w="14rem">{entry?.summary}</SkText>
        </div>
        <div class="list-row-details">
          <SkBlock w="0.75rem" h="0.75rem" circle>
            <span class={`memory-importance-dot ${importanceDotClass(entry?.importance ?? 0)}`}
              data-tooltip={`Importance: ${(entry?.importance ?? 0).toFixed(2)}`} />
          </SkBlock>
          <SkBlock w="3.5rem" h="1.25rem" round>
            <span class={`memory-source-badge ${entry?.source.type}`}>
              {entry?.source.type === 'event' ? 'Event' : 'Artifact'}
            </span>
          </SkBlock>
          {(sk || entry) && (
            <SkText w="5rem">
              {entry && (
                <span data-tooltip={formatDateTime(new Date(entry.src_created_at))}>
                  {formatTimeAgo(new Date(entry.src_created_at))}
                </span>
              )}
            </SkText>
          )}
        </div>

        <Disclosure open={!sk && expanded && !!entry}>
          {entry && (
            <div class="memory-entry-details">
              <div class="memory-detail-row">
                <span class="memory-detail-label">ID:</span>
                <button
                  type="button"
                  class="memory-id-value"
                  data-tooltip="Copy id"
                  onClick={(e) => {
                    e.stopPropagation();
                    copyToClipboard(entry.id, 'Memory id copied');
                  }}
                >
                  {entry.id}
                </button>
              </div>
              <div class="memory-detail-row">
                <span class="memory-detail-label">Importance:</span>
                <span>{entry.importance.toFixed(2)}</span>
              </div>
              {entry.entities.length > 0 && (
                <div class="memory-detail-row">
                  <span class="memory-detail-label">Entities:</span>
                  <span>{entry.entities.join(', ')}</span>
                </div>
              )}
              <div class="memory-detail-row">
                <span class="memory-detail-label">Source:</span>
                <span>
                  {entry.source.type === 'event'
                    ? `Event ${entry.source.id}`
                    : `${entry.source.path} @ ${entry.source.commit}`}
                </span>
              </div>
              <button
                class="action-btn memory-view-source-btn"
                onClick={(e) => { e.stopPropagation(); void toggleSource(); }}
              >
                {sourceVisible ? 'Hide source' : 'View source'}
              </button>
              {sourceData.status === 'failed' && (
                <div class="memory-source-error">Failed to load source: {sourceData.error}</div>
              )}
              {/* Rolls open with the skeleton once the read is slow, or with the
                  source itself on a fast one. */}
              <Disclosure open={sourceVisible && (sourceData.status === 'loaded' || showSourceLoading)}>
                <LoadingFade
                  showSkeleton={showSourceLoading}
                  skeleton={<SkeletonProvider><div class="memory-source-content"><SourceBlock /></div></SkeletonProvider>}
                >
                  {sourceData.status === 'loaded' && (
                    <div class="memory-source-content">
                      {sourceData.data.source_type === 'event' && sourceData.data.event && (
                        <SourceBlock text={JSON.stringify(sourceData.data.event.payload, null, 2)} />
                      )}
                      {sourceData.data.source_type === 'event' && !sourceData.data.event && (
                        <div class="memory-source-unavailable">Source event no longer available</div>
                      )}
                      {sourceData.data.source_type === 'artifact' && sourceData.data.artifact && (
                        <SourceBlock text={sourceData.data.artifact.content} />
                      )}
                    </div>
                  )}
                </LoadingFade>
              </Disclosure>
            </div>
          )}
        </Disclosure>
      </div>
    </div>
  );
}

export function MemoryInspector() {
  const [sourceFilter, setSourceFilter] = useState<string>('');
  const [sortBy, setSortBy] = useState<string>('created_at');
  const [importanceFilter, setImportanceFilter] = useState<Set<ImportanceLevel>>(new Set());
  const [offset, setOffset] = useState(0);
  const [forceRebuild, setForceRebuild] = useState(false);

  const stats = statsLoadable.value;
  const entries = entriesLoadable.value;
  const rebuild = memoryRebuildProgress.value;
  const statsPending = stats.status === 'not-loaded' || stats.status === 'loading';
  const entriesPending = entries.status === 'not-loaded' || entries.status === 'loading';
  // One gate for both reads, so their skeletons arrive in one wave. Each still
  // clears on its own read.
  const gate = useDelayedFlag(statsPending || entriesPending);
  const showStatsLoading = gate && statsPending;
  const showEntriesLoading = gate && entriesPending;

  const importanceParam = [...importanceFilter].join(',');

  // Numbers each read of the entries. A filter or page change can start a read
  // before the last one answers, and only the newest may land.
  const entriesRequest = useRef(0);
  // A re-read of the page on show keeps it visible. A new filter or page is
  // different entries, so it blanks rather than show the old ones as its own.
  const loadEntries = useCallback(async (newOffset: number, { reread = false }: { reread?: boolean } = {}) => {
    const request = ++entriesRequest.current;
    if (reread) setLoadingIfFresh(entriesLoadable);
    else entriesLoadable.value = { status: 'loading' };
    try {
      const data = await getMemoryEntries({
        limit: PAGE_SIZE,
        offset: newOffset,
        source_type: sourceFilter || undefined,
        sort: sortBy,
        importance: importanceParam || undefined,
      });
      if (request === entriesRequest.current) entriesLoadable.value = { status: 'loaded', data };
    } catch (e) {
      if (request === entriesRequest.current) entriesLoadable.value = toFailed(e);
    }
  }, [sourceFilter, sortBy, importanceParam]);

  const reread = () => Promise.all([
    loadStats(),
    loadEntries(offset, { reread: true }),
  ]);
  usePanelRefresh('memory', reread);
  useVersionedRefresh(memoryEntriesVersion.value, false, () => void reread());

  const prevRebuilding = useRef(false);

  useEffect(() => {
    if (stats.status === 'not-loaded') {
      void loadStats();
    }
  }, []);

  // Reload stats and entries when rebuild completes
  useEffect(() => {
    const isRebuilding = rebuild !== null;
    if (prevRebuilding.current && !isRebuilding) {
      void loadStats();
      void loadEntries(0);
      setOffset(0);
    }
    prevRebuilding.current = isRebuilding;
  }, [rebuild]);

  function toggleImportance(level: ImportanceLevel) {
    setImportanceFilter(prev => {
      const next = new Set(prev);
      if (next.has(level)) next.delete(level);
      else next.add(level);
      return next;
    });
  }

  useEffect(() => {
    setOffset(0);
    void loadEntries(0);
  }, [sourceFilter, sortBy, importanceParam, loadEntries]);

  function handleNextPage() {
    const newOffset = offset + PAGE_SIZE;
    setOffset(newOffset);
    void loadEntries(newOffset);
  }

  function handlePrevPage() {
    const newOffset = Math.max(0, offset - PAGE_SIZE);
    setOffset(newOffset);
    void loadEntries(newOffset);
  }

  function renderStats() {
    if (stats.status === 'failed') {
      return <LoadableError noun="stats" error={stats.error} />;
    }
    return (
      <LoadingFade showSkeleton={showStatsLoading} skeleton={<SkeletonProvider><MemoryStatsBar /></SkeletonProvider>}>
        {stats.status === 'loaded' && (
          <MemoryStatsBar stats={stats.data} importanceFilter={importanceFilter} onToggle={toggleImportance} />
        )}
      </LoadingFade>
    );
  }

  function renderEntries() {
    if (entries.status === 'failed') {
      return <LoadableError noun="entries" error={entries.error} />;
    }
    return (
      <LoadingFade showSkeleton={showEntriesLoading} skeleton={<ListSkeletonOf containerClass="list-rows" row={() => <MemoryEntryRow />} />}>
        {entries.status === 'loaded' ? (
          entries.data.entries.length === 0 ? (
            <div class="empty-state">No memory entries</div>
          ) : (
            <>
              <div class="list-rows">
                {entries.data.entries.map((entry) => (
                  <MemoryEntryRow key={entry.id} entry={entry} />
                ))}
              </div>
              <div class="memory-pagination">
                <span class="memory-pagination-info">
                  {offset + 1}–{Math.min(offset + PAGE_SIZE, entries.data.total)} of {entries.data.total.toLocaleString()}
                </span>
                <div class="memory-pagination-buttons">
                  <button class="action-btn" disabled={offset === 0} onClick={handlePrevPage}>Prev</button>
                  <button class="action-btn" disabled={!entries.data.has_more} onClick={handleNextPage}>Next</button>
                </div>
              </div>
            </>
          )
        ) : null}
      </LoadingFade>
    );
  }

  async function handleRebuild() {
    try {
      await rebuildMemory(forceRebuild);
      // Show immediate feedback before first SSE progress event arrives
      memoryRebuildProgress.value = { processed: 0, total: 0, percent: 0 };
    } catch (e) {
      showToast(`Failed to start rebuild: ${errorDetail(e)}`, 'error');
    }
  }

  async function handleCancel() {
    try {
      await cancelMemoryRebuild();
    } catch (e) {
      showToast(`Failed to cancel: ${errorDetail(e)}`, 'error');
    }
  }

  return (
    <>
      {renderStats()}
      <div class="memory-actions">
        {rebuild ? (
          <div class="memory-rebuild-progress">
            <div class="progress-bar">
              <div class="progress-bar-fill" style={{ width: `${rebuild.percent}%` }} />
            </div>
            <span class="progress-label" style="display: inline; margin-top: 0;">
              {rebuild.total === 0 ? 'Starting...' : `${rebuild.percent}% (${rebuild.processed}/${rebuild.total})`}
            </span>
            <button class="action-btn action-btn-danger" onClick={handleCancel}>Cancel</button>
          </div>
        ) : (
          <div class="memory-rebuild-controls">
            <button class="action-btn" onClick={handleRebuild}>Rebuild memory</button>
            <label class="memory-force-toggle">
              <input type="checkbox" checked={forceRebuild} onChange={(e) => setForceRebuild((e.target as HTMLInputElement).checked)} />
              Full rebuild
            </label>
          </div>
        )}
      </div>
      <div class="memory-filters">
        <Dropdown
          options={[
            { value: '', label: 'All sources' },
            { value: 'event', label: 'Events only' },
            { value: 'artifact', label: 'Artifacts only' },
          ]}
          value={sourceFilter}
          onChange={setSourceFilter}
        />
        <Dropdown
          options={[
            { value: 'created_at', label: 'Newest first' },
            { value: 'importance', label: 'Importance' },
          ]}
          value={sortBy}
          onChange={setSortBy}
        />
      </div>
      {renderEntries()}
    </>
  );
}
