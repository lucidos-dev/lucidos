import { useLayoutEffect, useRef } from 'preact/hooks';
import type { DiffFile } from '../../store/store';
import { diffFitsSideBySide } from '../../store/store';
import { changeBadgeLabel } from './changeBadge';
import { diffStats } from './diffStats';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';
import { getRemPx } from '../../utils/dom';
import { SideBySideDiff } from './sideBySideDiff';

interface Props {
  file: DiffFile;
  /** Render the two columns side by side instead of unified hunks. Honoured
   *  only when there is room for them (see `fitsSideBySide`). */
  sideBySide?: boolean;
  /** Measure this instance's width and publish it to `diffFitsSideBySide`,
   *  which is what the content-pane header reads to decide whether to OFFER the
   *  toggle.
   *
   *  Opt-in, and exactly one instance may: the file-preview diff is the single
   *  surface the header's controls act on, whereas `InlineDiffList` stacks one
   *  DiffView per changed file. N writers of one signal would be N-1 redundant
   *  ResizeObservers all racing to set the same value, measured off containers
   *  the header's toggle does not act on. Off by default, so a new caller
   *  cannot become a second writer by accident. */
  measureFit?: boolean;
}

/** The narrowest the diff can be and still show two columns, in `rem` so it
 *  tracks the user's UI scale rather than a hardcoded pixel count. Two gutters
 *  plus two columns of code: below this the columns are too narrow to read a
 *  line of source in without scrolling both of them constantly. */
export const SIDE_BY_SIDE_MIN_REM = 44;

/** Is there room for the side-by-side columns?
 *
 *  Pure, so the threshold is checkable without a DOM. Measured rather than
 *  guessed from the viewport because the content pane is resizable: a desktop
 *  user can drag the split until two columns no longer fit, and a phone never
 *  has room at all. A width of 0 is an unmeasured container (the first paint,
 *  before the ResizeObserver has run), which must not read as "no room" and
 *  flash the unified view before swapping. */
export function fitsSideBySide(widthPx: number, remPx: number): boolean {
  if (widthPx === 0) return true;
  return widthPx >= SIDE_BY_SIDE_MIN_REM * remPx;
}

export function DiffView({ file, sideBySide = false, measureFit = false }: Props) {
  const stats = diffStats(file);
  const rootRef = useRef<HTMLDivElement>(null);

  // The diff root is already the scroll container, so it is the element whose
  // width decides. Measured with a ResizeObserver rather than a CSS container
  // query because the fallback swaps RENDERERS: rendering both and letting CSS
  // pick one would double the DOM of a large diff. `getRemPx()` is read at
  // measure time, not captured, so a UI-scale change feeds straight in.
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root || !measureFit) return;
    const measure = () => { diffFitsSideBySide.value = fitsSideBySide(root.clientWidth, getRemPx()); };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(root);
    return () => ro.disconnect();
  }, [measureFit]);

  // Short-circuited, so an instance with the mode off never READS the signal and
  // therefore never subscribes to it: `InlineDiffList` mounts one DiffView per
  // changed file, and none of them can act on the fit.
  const showSideBySide = sideBySide && diffFitsSideBySide.value;

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

/** The unified rendering: hunks of `-`/`+`/context lines carrying both files'
 *  line numbers in one column. What the diff has always shown, and what a
 *  surface too narrow for two columns falls back to. */
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
