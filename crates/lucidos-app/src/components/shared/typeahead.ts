export function isTypeaheadKey(
  e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
): boolean {
  return e.key.length === 1 && e.key !== ' '
    && !e.metaKey && !e.ctrlKey && !e.altKey;
}

export function isTypeaheadSeedKey(
  e: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
  opts: { searching: boolean; freeText?: boolean },
): boolean {
  return !opts.freeText && !opts.searching && isTypeaheadKey(e);
}
