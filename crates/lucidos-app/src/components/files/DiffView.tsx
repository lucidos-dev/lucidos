import { useLayoutEffect, useRef } from 'preact/hooks';
import type { DiffFile } from '../../store/store';
import { diffFitsSideBySide } from '../../store/store';
import { changeBadgeLabel } from './changeBadge';
import { diffStats } from './diffStats';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { getRemPx } from '../../utils/dom';
import { SideBySideDiff } from './sideBySideDiff';
import { SkBlock, SkText, SkeletonProvider } from '../shared/Skeleton';

interface Props {
  file?: DiffFile;
  sideBySide?: boolean;
  measureFit?: boolean;
}

export const SIDE_BY_SIDE_MIN_REM = 44;

export function fitsSideBySide(widthPx: number, remPx: number): boolean {
  if (widthPx === 0) return true;
  return widthPx >= SIDE_BY_SIDE_MIN_REM * remPx;
}

const DIFF_SKELETON_LINES = ['62%', '80%', '48%', '71%', '35%'];

export function DiffSkeleton({ files = 1, class: cls }: { files?: number; class?: string }) {
  return (
    <SkeletonProvider>
      <div class={cls} aria-hidden="true">
        {Array.from({ length: files }, (_, i) => <DiffView key={i} />)}
      </div>
    </SkeletonProvider>
  );
}

export function DiffView({ file, sideBySide = false, measureFit = false }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root || !measureFit) return;
    const measure = () => { diffFitsSideBySide.value = fitsSideBySide(root.clientWidth, getRemPx()); };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(root);
    return () => ro.disconnect();
  }, [measureFit]);

  const showSideBySide = sideBySide && diffFitsSideBySide.value;

  if (!file) {
    return (
      <div class="diff-view">
        <div class="diff-header">
          <SkBlock w="3.5rem" h="1rem" round />
          <SkText class="diff-path" w="14rem" />
        </div>
        <div class="diff-hunk">
          <div class="diff-hunk-header"><SkText w="9rem" /></div>
          {DIFF_SKELETON_LINES.map((w) => (
            <div key={w} class="diff-line diff-line-context">
              <span class="diff-line-num diff-line-num-old" />
              <span class="diff-line-num diff-line-num-new" />
              <span class="diff-line-marker"> </span>
              <SkText class="diff-line-content" w={w} />
            </div>
          ))}
        </div>
      </div>
    );
  }
  const stats = diffStats(file);
  return (
    <div class="diff-view" ref={rootRef}>
      <div class="diff-header">
        <span class={`change-badge change-badge-${file.status}`}>
          {changeBadgeLabel(file.status)}
        </span>
        <span class="diff-path">{file.path}</span>
        <DiffStatsInline additions={stats.additions} deletions={stats.deletions} />
      </div>
      {showSideBySide ? <SideBySideDiff file={file} /> : <UnifiedHunks file={file} />}
    </div>
  );
}

function UnifiedHunks({ file }: { file: DiffFile }) {
  const ext = file.path.split('.').pop()?.toLowerCase() || '';
  const isCode = CODE_EXTS.includes(ext);

  return (
    <>
      {file.hunks.map((hunk, i) => {
        let oldLine = hunk.old_start;
        let newLine = hunk.new_start;

        const hunkText = hunk.lines.map(l => l.content).join('\n');
        const highlighted = isCode
          ? highlightFileLines(hunkText, ext)
          : hunkText.split('\n').map(escapeHtml);

        return (
          <div key={i} class="diff-hunk">
            <div class="diff-hunk-header">
              @@ -{hunk.old_start},{hunk.old_count} +{hunk.new_start},{hunk.new_count} @@
            </div>
            {hunk.lines.map((line, j) => {
              let leftNum = '';
              let rightNum = '';

              if (line.type === 'context') {
                leftNum = String(oldLine++);
                rightNum = String(newLine++);
              } else if (line.type === 'deletion') {
                leftNum = String(oldLine++);
              } else if (line.type === 'addition') {
                rightNum = String(newLine++);
              }

              return (
                <div key={j} class={`diff-line diff-line-${line.type}`}>
                  <span class="diff-line-num diff-line-num-old">{leftNum}</span>
                  <span class="diff-line-num diff-line-num-new">{rightNum}</span>
                  <span class="diff-line-marker">
                    {line.type === 'addition' ? '+' : line.type === 'deletion' ? '-' : ' '}
                  </span>
                  <span class="diff-line-content" dangerouslySetInnerHTML={{ __html: highlighted[j] }} />
                </div>
              );
            })}
          </div>
        );
      })}
    </>
  );
}

export function DiffStatsInline({ additions, deletions }: { additions: number; deletions: number }) {
  if (additions === 0 && deletions === 0) return null;
  return (
    <span class="diff-stats">
      {additions > 0 && <span class="diff-stat-add">+{additions}</span>}
      {deletions > 0 && <span class="diff-stat-del">-{deletions}</span>}
    </span>
  );
}
