
import type { DropdownOption } from '../shared/Dropdown';
import {
  destinationToOptionValue,
  LUCIDOS_AGENT_BLURB,
  REGISTER_REPO_OPTION_VALUE,
  type ComposeDestination,
} from '../../store/composeDestination';

export type DestinationOptionInputs = {
  dest: ComposeDestination;
  apps: readonly { id: string; name: string }[];
  externalRepos: readonly { id: string; name: string }[];
  packaged: boolean;
  listsPending: boolean;
  reposError: string | null;
  appsError: string | null;
  users: readonly { id: string; name: string; has_device: boolean }[];
};

export function buildDestinationOptions(inputs: DestinationOptionInputs): DropdownOption[] {
  const { dest, apps, externalRepos, packaged, listsPending, reposError, appsError, users } = inputs;

  const options: DropdownOption[] = [
    {
      value: destinationToOptionValue({ kind: 'lucidos-agent' }),
      label: 'Lucidos Agent',
      description: LUCIDOS_AGENT_BLURB,
    },
  ];
  if (users.length > 0) {
    options.push({ value: '__hdr-users', label: 'A teammate…', disabled: true });
    for (const p of users) {
      options.push({
        value: destinationToOptionValue({ kind: 'user', userId: p.id }),
        label: p.name,
        description: p.has_device ? 'Starts with the agent paused' : 'No paired device, so no push reaches them',
      });
    }
  }
  options.push({ value: '__hdr-coding', label: 'Coding agent on…', disabled: true });
  if (!packaged) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'lucidos' } }),
      label: 'Lucidos source',
      description: "The Lucidos platform's own code",
    });
  }
  for (const r of externalRepos) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'external', repoId: r.id } }),
      label: `${r.name} · repository`,
    });
  }
  for (const a of apps) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'app', appId: a.id } }),
      label: `${a.name} · app`,
    });
  }
  if (listsPending) {
    options.push({ value: '__loading', label: 'Loading…', disabled: true });
  }
  if (reposError !== null) {
    options.push({
      value: '__repos-error',
      label: 'Failed to load repositories',
      description: reposError,
      disabled: true,
      danger: true,
    });
  }
  if (appsError !== null) {
    options.push({
      value: '__apps-error',
      label: 'Failed to load apps',
      description: appsError,
      disabled: true,
      danger: true,
    });
  }
  options.push({ value: REGISTER_REPO_OPTION_VALUE, label: '＋ Register a repository…' });

  const value = destinationToOptionValue(dest);
  if (!listsPending && !options.some(o => o.value === value)) {
    const codingScope = dest.kind === 'coding' ? dest.scope : null;
    const unavailableName =
      dest.kind === 'user' ? 'That teammate'
      : codingScope?.kind === 'lucidos' ? 'Lucidos source'
      : codingScope?.kind === 'app' ? codingScope.appId
      : codingScope?.kind === 'external' ? codingScope.repoId
      : value;
    options.push({
      value,
      label: `${unavailableName} · unavailable`,
      disabled: true,
      danger: true,
    });
  }

  return options;
}
