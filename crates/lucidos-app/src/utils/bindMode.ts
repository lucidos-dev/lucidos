
export type BindMode = 'loopback' | 'address' | 'all';

export interface BindDraft {
  mode: BindMode;
  address: string;
}

export function draftFromBind(bind: string): BindDraft {
  return parseBindValue(bind);
}

export function bindDraftMatchesSaved(draft: BindDraft, savedBind: string): boolean {
  const saved = draftFromBind(savedBind);
  return toBindValue(draft.mode, draft.address) === toBindValue(saved.mode, saved.address);
}

export function parseBindValue(value: string): { mode: BindMode; address: string } {
  const v = value.trim().toLowerCase();
  if (v === '' || v === 'loopback') return { mode: 'loopback', address: '' };
  if (v === 'all') return { mode: 'all', address: '' };
  return { mode: 'address', address: value.trim() };
}

export function toBindValue(mode: BindMode, address: string): string {
  if (mode === 'loopback') return 'loopback';
  if (mode === 'all') return 'all';
  return address.trim();
}

function isValidIpv4(value: string): boolean {
  const parts = value.split('.');
  if (parts.length !== 4) return false;
  return parts.every(
    (p) => /^\d{1,3}$/.test(p) && Number(p) <= 255 && String(Number(p)) === p,
  );
}

function isValidIpv6(value: string): boolean {
  if (!value.includes(':')) return false;
  if (!/^[0-9a-fA-F:]+$/.test(value)) return false;
  return (value.match(/::/g)?.length ?? 0) <= 1;
}

export function isValidIp(value: string): boolean {
  const v = value.trim();
  if (v === '') return false;
  return isValidIpv4(v) || isValidIpv6(v);
}

export function isValidBindSelection(mode: BindMode, address: string): boolean {
  return mode !== 'address' || isValidIp(address);
}
