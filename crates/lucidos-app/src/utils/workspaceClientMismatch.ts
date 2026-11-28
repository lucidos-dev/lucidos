import { SIGIL } from '../generated/gateway-constants';
import { parseWorkspaceRoute, type WorkspaceRoute } from './basePath';

const BEHIND = ['this-gateway', 'remote-gateway', 'remote-engine'] as const;
type Behind = typeof BEHIND[number];

export interface WorkspaceClientMismatch {
  route: WorkspaceRoute & { gateway: string };
  behind: Behind;
  ours: number;
  theirs: string | null;
  release: string | null;
  bindingChanged: boolean;
}

function parse(fields: Record<string, unknown>): WorkspaceClientMismatch | null {
  const route = typeof fields.incompatible === 'string' ? parseWorkspaceRoute(fields.incompatible) : null;
  const behind = BEHIND.find((side) => side === fields.behind);
  const ours = fields.ours;
  if (!route?.gateway || !behind || typeof ours !== 'number' || !Number.isInteger(ours) || ours < 0) return null;
  return {
    route: { ...route, gateway: route.gateway },
    behind,
    ours,
    theirs: typeof fields.theirs === 'string' && /^\d{1,10}-\d{1,10}$/.test(fields.theirs) ? fields.theirs : null,
    release: typeof fields.release === 'string' && /^[a-zA-Z0-9.+-]{1,64}$/.test(fields.release) ? fields.release : null,
    bindingChanged: fields.binding_changed === true,
  };
}

export function readWorkspaceClientMismatch(value: unknown): WorkspaceClientMismatch | null {
  if (!value || typeof value !== 'object' || !('error' in value) || value.error !== 'workspace_client_mismatch') return null;
  return parse(value as Record<string, unknown>);
}

export function mismatchFromPickerQuery(search: string): WorkspaceClientMismatch | null {
  const query = new URLSearchParams(search);
  const ours = query.get('ours');
  if (!query.has('pick') || !ours || !/^\d{1,10}$/.test(ours)) return null;
  return parse({
    incompatible: query.get('incompatible'),
    behind: query.get('behind'),
    ours: Number(ours),
    theirs: query.get('theirs'),
    release: query.get('release'),
    binding_changed: query.get('binding-changed') === '1',
  });
}

export function mismatchPickerHref(mismatch: WorkspaceClientMismatch): string {
  const { route, behind, ours, theirs, release, bindingChanged } = mismatch;
  const query = new URLSearchParams({
    incompatible: `${route.slug}@${route.gateway}`,
    behind,
    ours: String(ours),
    theirs: theirs ?? '',
  });
  if (release) query.set('release', release);
  if (bindingChanged) query.set('binding-changed', '1');
  return `/${SIGIL}/?pick&${query}`;
}
