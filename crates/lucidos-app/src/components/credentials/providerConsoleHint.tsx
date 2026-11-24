import { type VNode } from 'preact';
import type { KnownOAuthProvider } from '../../store/types';
import { rowForService } from './oauthClientForm';

export function ConsoleLink({ row }: { row: KnownOAuthProvider }): VNode | null {
  if (!row.console_url) return null;
  return (
    <a class="accent-link" href={row.console_url} target="_blank" rel="noopener noreferrer">
      Open the {row.console_label ?? `${row.label} console`} ↗
    </a>
  );
}

export function ProviderPermissionsHint({ row }: { row: KnownOAuthProvider }): VNode {
  return (
    <div class="oauth-permissions-hint">
      {row.permissions_hint && <p>{row.permissions_hint}</p>}
      <ConsoleLink row={row} />
    </div>
  );
}

export function reauthorizationHint(
  providers: KnownOAuthProvider[],
  provider: string,
  hasShortfall: boolean,
): KnownOAuthProvider | undefined {
  if (!hasShortfall) return undefined;
  const row = rowForService(providers, provider);
  if (!row) return undefined;
  return row.permissions_hint || row.console_url ? row : undefined;
}
