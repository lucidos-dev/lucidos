/**
 * Client-side helpers for the Network access controls (per-workspace engine bind
 * + machine-global gateway bind). The stored value is a single string:
 * `"loopback"`, `"all"`, or an IP literal. The UI presents it as a three-mode
 * radio (Loopback only / Tailnet or specific address / All interfaces) plus an
 * address field. These functions translate both directions and validate the IP
 * field. The engine/gateway validate authoritatively server-side — this is UX.
 */

export type BindMode = 'loopback' | 'address' | 'all';

/** An in-progress edit of a bind: the three-mode selection, the address text
 *  that only `address` mode uses, and the engine-inherit flag that rides along
 *  in the same form.
 *
 *  A draft is only ever meaningful next to the saved value it was seeded from,
 *  so holders must keep the two together (see `NetworkEditor` in
 *  `components/picker/NetworkAccessPopover.tsx`) rather than in independent
 *  fields: a draft that can outlive its config resurfaces as the "saved" value
 *  the next time the form opens. */
export interface BindDraft {
  mode: BindMode;
  address: string;
  inherit: boolean;
}

/** Seed a draft from a saved bind value. The only way to make one, so a draft
 *  always starts out equal to what is actually stored. */
export function draftFromBind(bind: string, inherit: boolean): BindDraft {
  const { mode, address } = parseBindValue(bind);
  return { mode, address, inherit };
}

/** Would saving this draft change anything?
 *
 *  Compares the PAYLOAD the write would send against the payload that
 *  reproduces the stored state, so both sides are canonical: keyword case and
 *  surrounding whitespace collapse, and the address is ignored in the modes
 *  that do not use it (typing an IP and then picking "all" is not a change).
 *  Save is offered only when this is false, so an enabled Save button always
 *  means "there is something to save". */
export function bindDraftMatchesSaved(
  draft: BindDraft,
  savedBind: string,
  savedInherit: boolean,
): boolean {
  const saved = draftFromBind(savedBind, savedInherit);
  return (
    draft.inherit === saved.inherit &&
    toBindValue(draft.mode, draft.address) === toBindValue(saved.mode, saved.address)
  );
}

/** Parse a stored bind value into a UI mode + (for `address`) the IP text. */
export function parseBindValue(value: string): { mode: BindMode; address: string } {
  const v = value.trim().toLowerCase();
  if (v === '' || v === 'loopback') return { mode: 'loopback', address: '' };
  if (v === 'all') return { mode: 'all', address: '' };
  return { mode: 'address', address: value.trim() };
}

/** Serialize a UI mode + address back to the stored value. */
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

/** Loose IPv6 check — hex groups + colons, at most one `::`. Good enough for UX;
 *  the engine parses authoritatively. */
function isValidIpv6(value: string): boolean {
  if (!value.includes(':')) return false;
  if (!/^[0-9a-fA-F:]+$/.test(value)) return false;
  return (value.match(/::/g)?.length ?? 0) <= 1;
}

/** Is `value` a syntactically valid IP literal (IPv4 or IPv6)? */
export function isValidIp(value: string): boolean {
  const v = value.trim();
  if (v === '') return false;
  return isValidIpv4(v) || isValidIpv6(v);
}

/** Is the (mode, address) selection submittable? Only `address` mode needs a
 *  valid IP; the keyword modes are always valid. */
export function isValidBindSelection(mode: BindMode, address: string): boolean {
  return mode !== 'address' || isValidIp(address);
}
