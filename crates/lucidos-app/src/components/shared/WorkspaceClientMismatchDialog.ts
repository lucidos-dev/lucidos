import { listRemoteGateways, type RemoteGateway } from '../../api/client/control';
import { showConfirm } from '../../store/store';
import { replaceDocument } from '../../utils/documentNavigation';
import type { WorkspaceClientMismatch } from '../../utils/workspaceClientMismatch';

const PAIRED_ADDRESS_TIMEOUT_MS = 2_000;

export function mismatchMessage(mismatch: WorkspaceClientMismatch): string {
  const { route: { slug, gateway }, behind, ours, theirs, release, bindingChanged } = mismatch;
  const local = `This gateway${release ? ` (${release})` : ''} speaks workspace-client contract ${ours}.`;
  const subject = `'${slug}' on '${gateway}'`;
  const contract = behind === 'remote-gateway' ? 'gateway contract' : 'workspace-client contract';
  const [min, max] = theirs?.split('-') ?? [];
  const remote = theirs
    ? `${subject} uses ${contract} ${min === max ? min : theirs}.`
    : `${subject} speaks an unknown contract version.`;
  const action = bindingChanged ? 'The remote contract changed while opening the workspace. Try opening it again.'
    : behind === 'this-gateway' ? 'Update this gateway.'
    : behind === 'remote-gateway' ? `Update the gateway on '${gateway}'.`
    : `Update the workspace engine for '${slug}' on '${gateway}'.`;
  return `${local} ${remote}\n\n${action}`;
}

export function directWorkspaceHref(mismatch: WorkspaceClientMismatch, paired: readonly Pick<RemoteGateway, 'name' | 'address'>[]): string | null {
  const remote = paired.find((item) => item.name === mismatch.route.gateway);
  if (!remote) return null;
  try {
    const origin = new URL(remote.address);
    if (origin.protocol !== 'https:' && origin.protocol !== 'http:') return null;
    return new URL(`/${encodeURIComponent(mismatch.route.slug)}/`, origin).href;
  } catch {
    return null;
  }
}

export async function showWorkspaceClientMismatch(mismatch: WorkspaceClientMismatch, stillHere = () => true): Promise<void> {
  const paired = await listRemoteGateways({
    signal: AbortSignal.timeout(PAIRED_ADDRESS_TIMEOUT_MS),
  }).catch(() => []);
  if (!stillHere()) return;
  const href = directWorkspaceHref(mismatch, paired);
  const { slug, gateway } = mismatch.route;
  const confirm = href ? `Open '${slug}' on ${gateway}` : 'Close';
  const open = await showConfirm(mismatchMessage(mismatch), confirm, {
    title: 'Incompatible workspace client',
    variant: 'default',
    cancelLabel: 'Close',
    acknowledge: href === null,
  });
  if (open && href && stillHere()) replaceDocument(href);
}
