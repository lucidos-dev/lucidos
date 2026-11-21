/** Above this the count is drawn as `99+`. The bell caps at 999, and can
 *  afford to: it is a full-size badge on a roomy icon. This one rides a menu
 *  row beside a name it must not squeeze, and the brand mark's corner. */
const MAX_SHOWN_COUNT = 99;

/** The count, capped. A module of its own, so the mark's badge reaches it
 *  without pulling the menu rows into the shell chunk. */
export function countLabel(count: number): string {
  return count > MAX_SHOWN_COUNT ? `${MAX_SHOWN_COUNT}+` : String(count);
}
