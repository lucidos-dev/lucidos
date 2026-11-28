const MAX_SHOWN_COUNT = 99;

export function countLabel(count: number): string {
  return count > MAX_SHOWN_COUNT ? `${MAX_SHOWN_COUNT}+` : String(count);
}
