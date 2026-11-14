import { useState } from 'preact/hooks';
import { Disclosure } from '../shared/Disclosure';

export interface HeaderDraft { name: string; value: string }

/** Preserve values exactly; reject duplicates before turning rows into an object. */
export function headersFromDraft(rows: HeaderDraft[]): Record<string, string> {
  const headers: Record<string, string> = Object.create(null);
  for (const row of rows) {
    const name = row.name.trim().toLowerCase();
    if (!name) throw new Error('Every header needs a name.');
    if (Object.prototype.hasOwnProperty.call(headers, name)) throw new Error(`Duplicate header name: ${name}`);
    headers[name] = row.value;
  }
  return headers;
}

/** Shared by Settings and the picker. Saved values are never read back. */
export function RemoteHeadersEditor({ rows, onChange, disabled = false }: {
  rows: HeaderDraft[];
  onChange: (rows: HeaderDraft[]) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  return <div>
    <button class="action-btn" aria-expanded={open} onClick={() => setOpen(!open)}>
      Extra request headers
    </button>
    <Disclosure open={open}>
      <p class="settings-row-note">Only if an access proxy sits in front. A tailnet is recommended: it exposes nothing publicly.</p>
      {rows.map((row, index) => <div class="settings-row" key={index}>
        <input class="settings-text-input" aria-label={`Header ${index + 1} name`} placeholder="Header name"
          value={row.name} disabled={disabled} spellcheck={false} autoComplete="off"
          onInput={e => onChange(rows.map((r, i) => i === index ? { ...r, name: e.currentTarget.value } : r))} />
        <input class="settings-text-input" aria-label={`Header ${index + 1} value`} placeholder="Value"
          type="password" value={row.value} disabled={disabled} autoComplete="off"
          onInput={e => onChange(rows.map((r, i) => i === index ? { ...r, value: e.currentTarget.value } : r))} />
        <button class="action-btn" disabled={disabled} onClick={() => onChange(rows.filter((_, i) => i !== index))}>Remove header</button>
      </div>)}
      <button class="action-btn" disabled={disabled} onClick={() => onChange([...rows, { name: '', value: '' }])}>Add header</button>
      <p class="settings-row-note">Values are sealed on this gateway and never shown again. The packaged macOS app uses Keychain; other installations use an owner-only key file.</p>
    </Disclosure>
  </div>;
}
