import type { DiffFile } from '../../store/store';
import { changeBadgeLabel } from './changeBadge';
import { diffStats } from './diffStats';
import { highlightFileLines, CODE_EXTS } from '../../utils/syntaxHighlight';
import { escapeHtml } from '../../utils/escapeHtml';

interface Props {
  file: DiffFile;
}

export function DiffView({ file }: Props) {
  const ext = file.path.split('.').pop()?.toLowerCase() || '';
  const isCode = CODE_EXTS.includes(ext);
  const stats = diffStats(file);

  return (
    <div class="diff-view">
      <div class="diff-header">
        <span class={`change-badge change-badge-${file.status}`}>
          {changeBadgeLabel(file.status)}
        </span>
        <span class="diff-path">{file.path}</span>
        <DiffStatsInline additions={stats.additions} deletions={stats.deletions} />
      </div>
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
    </div>
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
