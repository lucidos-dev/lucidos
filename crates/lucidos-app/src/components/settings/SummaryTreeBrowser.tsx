import { useState, useEffect, useRef, useCallback } from 'preact/hooks';
import { getSummaryTree, getSummaryTreeThreads, getRecallZoom, getRecallDate } from '../../api/client';
import { summaryTreesVersion } from '../../store/store';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Disclosure } from '../shared/Disclosure';
import { DisclosureChevron } from '../shared/DisclosureChevron';
import { Pager } from '../shared/Pager';
import { loadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { formatTimeAgo, formatDateTime } from '../../utils/formatTime';
import { errorDetail } from '../../utils/errorDetail';
import { storedThreadTitle } from '../../utils/threadTitle';
import { dateRangeLabel, entriesLabel, isSourceText, lineSpan, pendingNote } from './summaryTree';
import type {
  RecallDateResponse,
  RecallLine,
  SummaryTreeThread,
  SummaryTreeThreadsResponse,
  SummaryTreeTop,
} from '../../api/types';

export const THREADS_PAGE_SIZE = 50;

/** A read held in component state. Only the newest read lands, and a re-read
 *  keeps what is shown until it answers. */
function useLatestRead<T>(): [Loadable<T>, (read: () => Promise<T>, reread?: boolean) => Promise<void>] {
  const [loadable, setLoadable] = useState<Loadable<T>>({ status: 'not-loaded' });
  const request = useRef(0);
  const run = useCallback(async (read: () => Promise<T>, reread = false) => {
    const id = ++request.current;
    setLoadable(reread ? loadingIfFresh : { status: 'loading' });
    try {
      const data = await read();
      if (id === request.current) setLoadable({ status: 'loaded', data });
    } catch (e) {
      if (id === request.current) setLoadable(toFailed(e));
    }
  }, []);
  return [loadable, run];
}

interface Opened {
  lines: RecallLine[];
  /** The line's time range, or why it has none. A leaf whose source events
   *  are gone still opens; only its date is missing. */
  date: RecallDateResponse | { error: string };
}

function DateLabel({ date }: { date: Opened['date'] }) {
  if ('error' in date) return <span class="tree-line-date" data-tooltip={date.error}>No date</span>;
  return <span class="tree-line-date">{dateRangeLabel(date.from, date.to)}</span>;
}

/** Reports a view's count label to the browser's head row, which stays
 *  mounted across views so its tabs keep focus. */
function useCountLabel(label: string | null, onCount: (label: string | null) => void) {
  useEffect(() => { onCount(label); }, [label]);
}

/** One summary line. It opens one zoom level in place: into finer lines, or
 *  for a leaf into its source. With no `line`, inside a `SkeletonProvider`,
 *  it draws itself as a loading placeholder. */
function TreeLineRow({ line }: { line?: RecallLine }) {
  const sk = useSkeleton();
  const [expanded, setExpanded] = useState(false);
  const [opened, open] = useLatestRead<Opened>();
  const showLoading = useDelayedLoading(opened);
  const span = line ? lineSpan(line.id) : null;

  const read = useCallback(async (): Promise<Opened> => {
    const id = line!.id;
    const [zoom, date] = await Promise.allSettled([
      getRecallZoom(id),
      getRecallDate(id),
    ]);
    if (zoom.status === 'rejected') throw zoom.reason;
    return {
      lines: zoom.value.lines,
      date: date.status === 'fulfilled' ? date.value : { error: errorDetail(date.reason) },
    };
  }, [line?.id]);

  usePanelRefresh('summary tree line', opened.status === 'loaded' ? () => open(read, true) : null);

  if (line && span === null) {
    return <div class="tree-line-note">{pendingNote(line.text)}</div>;
  }

  function toggle() {
    if (!expanded && opened.status !== 'loaded' && opened.status !== 'loading') void open(read);
    setExpanded(!expanded);
  }

  const date = opened.status === 'loaded' ? opened.data.date : null;
  return (
    <div class="tree-line-item">
      <button
        type="button"
        class={`tree-line${expanded ? ' open' : ''}`}
        aria-expanded={sk ? undefined : expanded}
        tabIndex={sk ? -1 : undefined}
        onClick={sk ? undefined : toggle}
      >
        <span class="tree-line-chevron"><DisclosureChevron open={!sk && expanded} /></span>
        <span class="tree-line-body">
          <span class="tree-line-meta">
            <SkBlock w="4.5rem" h="1rem" round>
              <span class="tree-line-span">{span !== null && entriesLabel(span)}</span>
            </SkBlock>
            {date && <DateLabel date={date} />}
          </span>
          <SkText class="tree-line-text" w="18rem">{line?.text}</SkText>
        </span>
      </button>
      <Disclosure open={!sk && expanded && (opened.status === 'loaded' || opened.status === 'failed' || showLoading)}>
        {line && (
          opened.status === 'failed' ? (
            <div class="tree-line-children"><LoadableError noun="this line" error={opened.error} /></div>
          ) : (
            <LoadingFade
              showSkeleton={showLoading}
              skeleton={<ListSkeletonOf count={2} containerClass="tree-line-children" row={() => <TreeLineRow />} />}
            >
              {opened.status === 'loaded' && <OpenedLines id={line.id} lines={opened.data.lines} />}
            </LoadingFade>
          )
        )}
      </Disclosure>
    </div>
  );
}

function OpenedLines({ id, lines }: { id: string; lines: RecallLine[] }) {
  if (isSourceText(id, lines)) {
    return <pre class="memory-source-pre tree-line-source">{lines[0].text}</pre>;
  }
  return (
    <div class="tree-line-children">
      {lines.map((l) => <TreeLineRow key={l.id} line={l} />)}
    </div>
  );
}

/** The top of one tree: the workspace's, or a thread's. */
function TreeTop({ threadId, onCount }: { threadId?: string; onCount: (label: string | null) => void }) {
  const [top, readTop] = useLatestRead<SummaryTreeTop>();
  const showLoading = useDelayedLoading(top);
  const load = (reread = false) => readTop(() => { return getSummaryTree(threadId); }, reread);

  useEffect(() => { void load(); }, [threadId]);
  usePanelRefresh('summary tree', () => load(true));
  useVersionedRefresh(summaryTreesVersion.value, false, () => void load(true));
  useCountLabel(top.status !== 'loaded' ? null : threadId
    ? entriesLabel(top.data.entries)
    : `${entriesLabel(top.data.entries)} summarised`, onCount);

  if (top.status === 'failed') return <LoadableError noun="the summary tree" error={top.error} />;
  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf containerClass="tree-lines" row={() => <TreeLineRow />} />}>
      {top.status === 'loaded' && (top.data.lines.length === 0 ? (
        <div class="empty-state">{threadId ? 'This thread has no entries yet' : 'Nothing is summarised yet'}</div>
      ) : (
        <div class="tree-lines">
          {top.data.lines.map((l) => <TreeLineRow key={l.id} line={l} />)}
        </div>
      ))}
    </LoadingFade>
  );
}

/** A thread with a tree. With no `thread`, inside a `SkeletonProvider`, it
 *  draws itself as a loading placeholder. */
function TreeThreadRow({ thread, onOpen }: { thread?: SummaryTreeThread; onOpen?: () => void }) {
  const sk = useSkeleton();
  return (
    <div class={`list-row tree-thread-row${sk ? '' : ' clickable'}`} onClick={sk ? undefined : onOpen}>
      <div class="list-row-info">
        <SkText class="title list-row-name" w="12rem">
          {thread && storedThreadTitle(thread.title)}
        </SkText>
        <div class="list-row-details">
          <SkText w="6rem">{thread && `${entriesLabel(thread.summarised)} summarised`}</SkText>
          <SkText w="4rem">
            {thread && (
              <span data-tooltip={formatDateTime(new Date(thread.last_activity))}>
                {formatTimeAgo(new Date(thread.last_activity))}
              </span>
            )}
          </SkText>
        </div>
      </div>
    </div>
  );
}

function TreeThreadList({ offset, onOffset, onCount, onOpen }: {
  offset: number;
  onOffset: (offset: number) => void;
  onCount: (label: string | null) => void;
  onOpen: (thread: SummaryTreeThread) => void;
}) {
  const [page, readPage] = useLatestRead<SummaryTreeThreadsResponse>();
  const showLoading = useDelayedLoading(page);
  const load = (at: number, reread = false) =>
    readPage(() => { return getSummaryTreeThreads({ limit: THREADS_PAGE_SIZE, offset: at }); }, reread);

  useEffect(() => { void load(offset); }, [offset]);
  usePanelRefresh('summary tree threads', () => load(offset, true));
  useVersionedRefresh(summaryTreesVersion.value, false, () => void load(offset, true));
  useCountLabel(page.status === 'loaded'
    ? `${page.data.total.toLocaleString()} ${page.data.total === 1 ? 'thread' : 'threads'}`
    : null, onCount);

  // A delete can empty the page shown. Step back to the last page that holds threads.
  const strandedPage = page.status === 'loaded' && page.data.threads.length === 0 && offset > 0;
  useEffect(() => {
    if (strandedPage && page.status === 'loaded') {
      onOffset(Math.max(0, Math.floor((page.data.total - 1) / THREADS_PAGE_SIZE) * THREADS_PAGE_SIZE));
    }
  }, [strandedPage]);

  if (page.status === 'failed') return <LoadableError noun="threads" error={page.error} />;
  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf containerClass="list-rows" row={() => <TreeThreadRow />} />}>
      {page.status === 'loaded' && !strandedPage && (page.data.threads.length === 0 ? (
        <div class="empty-state">No threads have a summary tree yet</div>
      ) : (
        <>
          <div class="list-rows">
            {page.data.threads.map((t) => <TreeThreadRow key={t.thread_id} thread={t} onOpen={() => onOpen(t)} />)}
          </div>
          <Pager
            offset={offset}
            pageSize={THREADS_PAGE_SIZE}
            total={page.data.total}
            hasMore={page.data.has_more}
            onChange={onOffset}
          />
        </>
      ))}
    </LoadingFade>
  );
}

type Tab = 'workspace' | 'threads';

const TABS: { value: Tab; label: string }[] = [
  { value: 'workspace', label: 'Workspace' },
  { value: 'threads', label: 'Threads' },
];

/** Settings → System → Memory on the Tree module: the summary trees, opened
 *  line by line down to the exact message, through the recall tool's own
 *  zoom (ADR 0362). The workspace tree, or any thread's. */
export function SummaryTreeBrowser() {
  const [tab, setTab] = useState<Tab>('workspace');
  const [thread, setThread] = useState<SummaryTreeThread | null>(null);
  const [threadsOffset, setThreadsOffset] = useState(0);
  const [count, setCount] = useState<string | null>(null);

  function show(next: Tab, opened: SummaryTreeThread | null) {
    if (next === tab && opened === thread) return;
    setTab(next);
    setThread(opened);
    setCount(null);
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="memory:summary-trees">Summary trees</div>
      <div class="tree-browser-head">
        <div class="segmented-control" role="group" aria-label="Summary tree">
          {TABS.map((t) => (
            <button
              key={t.value}
              type="button"
              aria-pressed={tab === t.value}
              class={`segmented-btn ${tab === t.value ? 'active' : ''}`}
              onClick={() => show(t.value, null)}
            >
              {t.label}
            </button>
          ))}
        </div>
        <span class="tree-browser-count">{count}</span>
        {thread && (
          <div class="tree-browser-thread">
            <button type="button" class="action-btn action-btn-secondary" onClick={() => show('threads', null)}>
              Back
            </button>
            <span class="tree-browser-thread-title">{storedThreadTitle(thread.title)}</span>
          </div>
        )}
      </div>
      {tab === 'workspace' && <TreeTop onCount={setCount} />}
      {tab === 'threads' && !thread && (
        <TreeThreadList
          offset={threadsOffset}
          onOffset={setThreadsOffset}
          onCount={setCount}
          onOpen={(t) => show('threads', t)}
        />
      )}
      {thread && <TreeTop key={thread.thread_id} threadId={thread.thread_id} onCount={setCount} />}
    </div>
  );
}
