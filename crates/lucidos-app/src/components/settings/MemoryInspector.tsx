import { useState, useEffect, useCallback, useRef } from 'preact/hooks';
import { signal } from '@preact/signals';
import { getMemoryStats, getMemoryEntries, getMemorySource, rebuildMemory, cancelMemoryRebuild } from '../../api/client';
import { showToast, memoryRebuildProgress } from '../../store/store';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { toFailed } from '../../store/types';
import { formatTimeAgo, formatDateTime } from '../../utils/formatTime';
import { errorDetail } from '../../utils/errorDetail';
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
  statsLoadable.value = { status: 'loading' };
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

function importanceDotClass(importance: number): string {
  if (importance >= 0.8) return 'critical';
  if (importance >= 0.6) return 'high';
  if (importance >= 0.3) return 'medium';
  return 'low';
}

function MemoryEntryRow({ entry }: { entry: MemoryEntryInfo }) {
  const [expanded, setExpanded] = useState(false);
  const [sourceData, setSourceData] = useState<Loadable<MemorySourceResponse>>({ status: 'not-loaded' });
  const [sourceVisible, setSourceVisible] = useState(false);

  async function toggleSource() {
    if (sourceData.status === 'loaded') {
      setSourceVisible(!sourceVisible);
      return;
    }
    if (sourceData.status === 'loading') return;
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
    <div class={`list-row memory-entry-row ${expanded ? 'expanded' : ''}`} onClick={() => setExpanded(!expanded)}>
      <div class="list-row-info" style={{ cursor: 'pointer' }}>
        <div class="title list-row-name memory-entry-summary">
          <span class="memory-topic-badge">{entry.topic}</span>
          <span class="memory-summary-text">{entry.summary}</span>
        </div>
        <div class="list-row-details">
          <span class={`memory-importance-dot ${importanceDotClass(entry.importance)}`}
            data-tooltip={`Importance: ${entry.importance.toFixed(2)}`} />
          <span class={`memory-source-badge ${entry.source.type}`}>
            {entry.source.type === 'event' ? 'Event' : 'Artifact'}
          </span>
          <span data-tooltip={formatDateTime(new Date(entry.src_created_at))}>
            {formatTimeAgo(new Date(entry.src_created_at))}
          </span>
        </div>

        {expanded && (
          <div class="memory-entry-details">
            <div class="memory-detail-row">
              <span class="memory-detail-label">ID:</span>
              <button
                type="button"
                class="memory-id-value"
                data-tooltip="Copy id"
                onClick={(e) => {
                  e.stopPropagation();
                  navigator.clipboard?.writeText(entry.id).then(
                    () => showToast('Memory id copied', 'success'),
                    () => showToast('Could not copy memory id', 'error'),
                  );
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
              {sourceData.status === 'loading' ? 'Loading...' : sourceVisible ? 'Hide source' : 'View source'}
            </button>
            {sourceData.status === 'failed' && (
              <div class="memory-source-error">Failed to load source: {sourceData.error}</div>
            )}
            {sourceVisible && sourceData.status === 'loaded' && (
              <div class="memory-source-content">
                {sourceData.data.source_type === 'event' && sourceData.data.event && (
                  <pre class="memory-source-pre">{JSON.stringify(sourceData.data.event.payload, null, 2)}</pre>
                )}
                {sourceData.data.source_type === 'event' && !sourceData.data.event && (
                  <div class="memory-source-unavailable">Source event no longer available</div>
                )}
                {sourceData.data.source_type === 'artifact' && sourceData.data.artifact && (
                  <pre class="memory-source-pre">{sourceData.data.artifact.content}</pre>
                )}
              </div>
            )}
          </div>
        )}
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
  const showStatsLoading = useDelayedLoading(stats);
  const showEntriesLoading = useDelayedLoading(entries);

  const importanceParam = [...importanceFilter].join(',');

  const loadEntries = useCallback(async (newOffset: number) => {
    entriesLoadable.value = { status: 'loading' };
    try {
      const data = await getMemoryEntries({
        limit: PAGE_SIZE,
        offset: newOffset,
        source_type: sourceFilter || undefined,
        sort: sortBy,
        importance: importanceParam || undefined,
      });
      entriesLoadable.value = { status: 'loaded', data };
    } catch (e) {
      entriesLoadable.value = toFailed(e);
    }
  }, [sourceFilter, sortBy, importanceParam]);

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
    if (stats.status !== 'loaded') {
      if (!showStatsLoading) return null;
      return <div class="loading-spinner" />;
    }
    const s = stats.data;
    const d = s.importance_distribution;
    const hasFilter = importanceFilter.size > 0 && importanceFilter.size < 4;
    const filteredTotal = hasFilter
      ? (importanceFilter.has('low') ? d.low : 0)
        + (importanceFilter.has('medium') ? d.medium : 0)
        + (importanceFilter.has('high') ? d.high : 0)
        + (importanceFilter.has('critical') ? d.critical : 0)
      : s.total;
    return (
      <div class="memory-stats-bar">
        <div class="memory-stat">
          <span class="memory-stat-value">{filteredTotal.toLocaleString()}</span>
          <span class="memory-stat-label">{hasFilter ? 'Filtered' : 'Total'}</span>
        </div>
        <div class="memory-stat">
          <span class="memory-stat-value">{s.event_count.toLocaleString()}</span>
          <span class="memory-stat-label">Events</span>
        </div>
        <div class="memory-stat">
          <span class="memory-stat-value">{s.artifact_count.toLocaleString()}</span>
          <span class="memory-stat-label">Artifacts</span>
        </div>
        <ImportanceBar distribution={d} selected={importanceFilter} onToggle={toggleImportance} />
      </div>
    );
  }

  function renderEntries() {
    if (entries.status === 'failed') {
      return <LoadableError noun="entries" error={entries.error} />;
    }
    if (entries.status !== 'loaded') {
      if (!showEntriesLoading) return null;
      return <div class="loading-spinner" />;
    }
    if (entries.data.entries.length === 0) {
      return <div class="empty-state">No memory entries</div>;
    }
    return (
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
