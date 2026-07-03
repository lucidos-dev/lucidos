/** Pure builder for the compose destination picker's option list. Lives apart
 *  from `ComposeDestinationRow.tsx` so the option set — including the
 *  packaged-build gate on "Lucidos source" — is unit-testable without rendering
 *  the component. The component derives the inputs (apps/repos Loadables,
 *  packaged flag, current destination) and feeds them here. */

import type { DropdownOption } from '../shared/Dropdown';
import {
  destinationToOptionValue,
  LUCIDOS_AGENT_BLURB,
  REGISTER_REPO_OPTION_VALUE,
  type ComposeDestination,
} from '../../store/composeDestination';

export type DestinationOptionInputs = {
  /** The current destination — used to detect a restored target the lists no
   *  longer contain (rendered as an explicit "· unavailable" row). */
  dest: ComposeDestination;
  apps: readonly { id: string; name: string }[];
  /** Registered repos minus the Lucidos-source row (the caller filters it out;
   *  the Lucidos source is offered as its own option, not a generic repo). */
  externalRepos: readonly { id: string; name: string }[];
  /** True on a packaged desktop build — no Lucidos source checkout exists, so
   *  "Lucidos source" would spawn against a non-existent repo and 500. Hidden
   *  there; apps + external repos still work (they don't need the source tree). */
  packaged: boolean;
  /** Apps or repos still loading — surfaced as an explicit pending row so the
   *  open menu never reads as "no apps / no repos". */
  listsPending: boolean;
  /** Error text when the repos list failed to load (null = no error). */
  reposError: string | null;
  /** Error text when the apps list failed to load (null = no error). */
  appsError: string | null;
};

export function buildDestinationOptions(inputs: DestinationOptionInputs): DropdownOption[] {
  const { dest, apps, externalRepos, packaged, listsPending, reposError, appsError } = inputs;

  const options: DropdownOption[] = [
    {
      value: destinationToOptionValue({ kind: 'lucidos-agent' }),
      label: 'Lucidos Agent',
      description: LUCIDOS_AGENT_BLURB,
    },
    { value: '__hdr-coding', label: 'Coding agent on…', disabled: true },
  ];
  // "Lucidos source" edits the platform's own code — only meaningful on a dev
  // build with a source checkout. A packaged install has none (is_packaged()),
  // so picking it spawns against an unregistered repo and 500s
  // ("Lucidos repo not registered — cannot classify folder"). Hide it there.
  if (!packaged) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'lucidos' } }),
      label: 'Lucidos source',
      description: "The Lucidos platform's own code",
    });
  }
  for (const a of apps) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'app', appId: a.id } }),
      label: `${a.name} · app`,
    });
  }
  for (const r of externalRepos) {
    options.push({
      value: destinationToOptionValue({ kind: 'coding', scope: { kind: 'external', repoId: r.id } }),
      label: `${r.name} · repository`,
    });
  }
  // Loading must not look like "no apps / no repos".
  if (listsPending) {
    options.push({ value: '__loading', label: 'Loading…', disabled: true });
  }
  // Failed must look different from empty — each list gets its own error row.
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

  // A restored coding target whose app/repo no longer exists (or a Lucidos
  // source target on a packaged build) would render the loading placeholder
  // forever — surface it as an explicit unavailable row once the lists settle.
  const value = destinationToOptionValue(dest);
  if (!listsPending && !options.some(o => o.value === value)) {
    const codingScope = dest.kind === 'coding' ? dest.scope : null;
    const unavailableName =
      codingScope?.kind === 'lucidos' ? 'Lucidos source'
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
