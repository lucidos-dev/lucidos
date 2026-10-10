/** A paged list's foot: which rows show ("51–100 of 1,234") and Prev/Next. */
export function Pager({ offset, pageSize, total, hasMore, onChange }: {
  offset: number;
  pageSize: number;
  total: number;
  hasMore: boolean;
  onChange: (offset: number) => void;
}) {
  return (
    <div class="pager">
      <span class="pager-info">
        {offset + 1}–{Math.min(offset + pageSize, total)} of {total.toLocaleString()}
      </span>
      <div class="pager-buttons">
        <button class="action-btn" disabled={offset === 0} onClick={() => onChange(Math.max(0, offset - pageSize))}>Prev</button>
        <button class="action-btn" disabled={!hasMore} onClick={() => onChange(offset + pageSize)}>Next</button>
      </div>
    </div>
  );
}
