import type { CredentialInfo, Loadable } from '../../store/types';
import {
  PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI_KEY,
  PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM_KEY,
  PROVIDER_ENABLED_TYPESAFE_KEY,
  SYSTEM_ONE_CUSTOM_MODEL_KEY,
  SYSTEM_ONE_CUSTOM_URL_KEY,
  switchValueReadsAsOff,
  type BackgroundModelKey,
  type BackgroundReasoningKey,
} from '../../store/actions/preferences';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice } from '../../store/modelSelection';
import { findProviderCredential } from './providerCredential';
import {
  CLOUDFLARE_ACCOUNTS_PREFIX,
  CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
  SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
  SYSTEM_ONE_ENDPOINT_IDS,
  TYPESAFE_CREDENTIAL_SERVICE,
} from '@lucidos/engine-constants';

/**
 * Which backend answers a judgment site, decided in the site's model control.
 *
 * Every System One row is a row in the model step, beside the chat models
 * (ADR 0224, widened by ADR 0363). This module decides what the field shows,
 * which rows it offers, and what a pick writes.
 *
 * Mirrors `llm::judgment::{endpoint, select}` in the engine, which read the
 * same ids out of the same preferences. The two must agree, or the field shows
 * a backend the engine does not use.
 */

/** The default, and what every unrecognized value resolves to. */
export const CHAT = 'chat';

/** The launch-environment fallback for TypeSafe's key, named in the UI because
 *  the page cannot see whether it is set. */
export { TYPESAFE_API_KEY_ENV } from '@lucidos/engine-constants';

/** Where every System One provider's own row is. */
export const SYSTEM_ONE_PROVIDER_LOCATION = 'Models → Providers';

/** One System One provider: a row on Settings → Models → Providers with its
 *  own key and master switch. */
export type SystemOneProviderId = 'typesafe' | 'cloudflare-workers-ai' | 'system-one-custom';

export interface SystemOneProviderSpec {
  id: SystemOneProviderId;
  label: string;
  /** Search / deep-link anchor of its provider row. */
  anchor: string;
  /** The credential service holding its key. */
  credentialService: string;
  switchKey: string;
}

export const SYSTEM_ONE_PROVIDERS: Record<SystemOneProviderId, SystemOneProviderSpec> = {
  typesafe: {
    id: 'typesafe',
    label: 'TypeSafe (Jev)',
    anchor: 'models:typesafe',
    credentialService: TYPESAFE_CREDENTIAL_SERVICE,
    switchKey: PROVIDER_ENABLED_TYPESAFE_KEY,
  },
  'cloudflare-workers-ai': {
    id: 'cloudflare-workers-ai',
    label: 'Cloudflare Workers AI (Clef)',
    anchor: 'models:cloudflare-workers-ai',
    credentialService: CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
    switchKey: PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI_KEY,
  },
  'system-one-custom': {
    id: 'system-one-custom',
    label: 'Custom System One endpoint',
    anchor: 'models:system-one-custom',
    credentialService: SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
    switchKey: PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM_KEY,
  },
};

/** One System One row a site can pick. */
export interface SystemOneEndpointSpec {
  /** The value a `judgment_*` preference stores. */
  id: (typeof SYSTEM_ONE_ENDPOINT_IDS)[number];
  provider: SystemOneProviderId;
  /** Its row in the model step. The value is never sent anywhere: the pick
   *  handler reads it and writes the `judgment_*` preference instead. Each is
   *  deliberately unlike every real model id, so no curated model collides. No
   *  reasoning tiers, so the picker settles in one step. */
  choice: ModelChoice;
}

const TYPED = 'Typed judgment rather than a written answer';

export const SYSTEM_ONE_ENDPOINTS: readonly SystemOneEndpointSpec[] = [
  {
    id: 'jev',
    provider: 'typesafe',
    choice: { value: 'typesafe-jev', label: 'TypeSafe (Jev)', description: TYPED, reasoningEfforts: [] },
  },
  {
    id: 'clef',
    provider: 'cloudflare-workers-ai',
    choice: { value: 'cloudflare-clef', label: 'Cloudflare Clef', description: TYPED, reasoningEfforts: [] },
  },
  {
    id: 'clef-flash',
    provider: 'cloudflare-workers-ai',
    choice: {
      value: 'cloudflare-clef-flash',
      label: 'Cloudflare Clef-flash',
      description: 'Fast typed judgment',
      reasoningEfforts: [],
    },
  },
  {
    id: 'custom',
    provider: 'system-one-custom',
    choice: { value: 'system-one-custom', label: 'Custom System One', description: TYPED, reasoningEfforts: [] },
  },
];

export type JudgmentSite = 'command-guard' | 'query-classification';

/** Every site with a control, in the order the provider rows list them. */
export const JUDGMENT_SITES: readonly JudgmentSite[] = ['command-guard', 'query-classification'];

export const JUDGMENT_PREFERENCE_KEY: Record<JudgmentSite, string> = {
  'command-guard': 'judgment_command_guard',
  'query-classification': 'judgment_query_classification',
};

/** The model half of the site's *model selection*, used while it runs on chat.
 *  Left alone by a System One pick, so switching back restores the same model. */
export const JUDGMENT_MODEL_KEY: Record<JudgmentSite, BackgroundModelKey> = {
  'command-guard': 'model_command_judge',
  'query-classification': 'model_query_classification',
};

/** The effort half, written with the model and never on its own. */
export const JUDGMENT_REASONING_KEY: Record<JudgmentSite, BackgroundReasoningKey> = {
  'command-guard': 'reasoning_command_judge',
  'query-classification': 'reasoning_query_classification',
};

/** What each site is called on screen. */
export const JUDGMENT_SITE_LABEL: Record<JudgmentSite, string> = {
  'command-guard': 'Command guard',
  'query-classification': 'Query classification',
};

/** Where its control is, for a provider row to point at. */
export const JUDGMENT_SITE_LOCATION: Record<JudgmentSite, string> = {
  'command-guard': 'Permissions → Command safety',
  'query-classification': 'Models → Background tasks',
};

/**
 * The System One row a stored preference value picks, or `null` for chat.
 *
 * Forgiving of case and surrounding space, strict about everything else, which
 * is exactly what the engine's `SystemOneEndpoint::from_id` does.
 */
export function pickedEndpoint(value: string | undefined | null): SystemOneEndpointSpec | null {
  const id = value?.trim().toLowerCase();
  return SYSTEM_ONE_ENDPOINTS.find((e) => e.id === id) ?? null;
}

/**
 * Whether a provider is set up, from what this page can see.
 *
 * TypeSafe and Cloudflare need a stored key. The custom endpoint needs a URL
 * and a model, as the engine does; its key is optional, because a self-hosted
 * model often takes none.
 */
export function systemOneConfigured(
  provider: SystemOneProviderId,
  credentials: Loadable<CredentialInfo[]>,
  prefs: Loadable<Record<string, string>>,
): boolean {
  if (provider === 'system-one-custom') {
    return prefs.status === 'loaded'
      && !!prefs.data[SYSTEM_ONE_CUSTOM_URL_KEY]?.trim()
      && !!prefs.data[SYSTEM_ONE_CUSTOM_MODEL_KEY]?.trim();
  }
  return !!findProviderCredential(credentials, SYSTEM_ONE_PROVIDERS[provider].credentialService);
}

/**
 * Whether a provider's master switch has been turned off.
 *
 * Absent means on, so this is false both for a workspace that never touched the
 * switch and for one still loading. What counts as off is the engine's own
 * vocabulary, read through `switchValueReadsAsOff`.
 */
export function systemOneSwitchedOff(
  prefs: Loadable<Record<string, string>>,
  provider: SystemOneProviderId,
): boolean {
  return prefs.status === 'loaded'
    && switchValueReadsAsOff(prefs.data[SYSTEM_ONE_PROVIDERS[provider].switchKey]);
}

/**
 * One site's stored preference value, or `undefined` while preferences load.
 *
 * Takes the `Loadable` rather than reading the store, so the decision stays
 * testable without mounting anything.
 */
export function storedJudgmentValue(
  prefs: Loadable<Record<string, string>>,
  site: JudgmentSite,
): string | undefined {
  return prefs.status === 'loaded' ? prefs.data[JUDGMENT_PREFERENCE_KEY[site]] : undefined;
}

/** What the agent's own route to Jev is called on screen. */
export const JUDGE_TOOL_LABEL = 'The agent’s judge tool';

/**
 * Everything using one provider right now, for its row's status line.
 *
 * **The judge tool is listed for TypeSafe on a stored key alone, because it has
 * no control.** A key plus the master switch on IS its whole condition (ADR
 * 0223), where a site needs a row picked in its model control. The row draws
 * only inside the open fold, which a switched-off provider never is.
 */
export function systemOneConsumers(
  provider: SystemOneProviderId,
  prefs: Loadable<Record<string, string>>,
  keyStored: boolean,
): string[] {
  const sites = JUDGMENT_SITES
    .filter((site) => pickedEndpoint(storedJudgmentValue(prefs, site))?.provider === provider)
    .map((site) => JUDGMENT_SITE_LABEL[site]);
  return provider === 'typesafe' && keyStored ? [JUDGE_TOOL_LABEL, ...sites] : sites;
}

/**
 * The System One rows this site's model step offers.
 *
 * **Unconfigured means absent**, which is how `chatModelOptions` treats a model
 * whose provider holds no key, and a switched-off provider offers none either.
 *
 * **The row a site is already on is always offered.** The engine also reads
 * `TYPESAFE_API_KEY` from the launch environment, which this page cannot see.
 * Drop the row there and the field renders a selection with no way back. That
 * is the one state a picker must never reach.
 */
export function offeredEndpoints(args: {
  picked: SystemOneEndpointSpec | null;
  configured: (provider: SystemOneProviderId) => boolean;
  switchedOff: (provider: SystemOneProviderId) => boolean;
}): SystemOneEndpointSpec[] {
  return SYSTEM_ONE_ENDPOINTS.filter((endpoint) =>
    endpoint.id === args.picked?.id
    || (args.configured(endpoint.provider) && !args.switchedOff(endpoint.provider)));
}

/** The heading over the System One rows, in a picker that has sections. */
export const SYSTEM_ONE_SECTION = 'System One';

/** The site's model rows: the background models, then every offered System
 *  One row, under its own heading when the background models have them. */
export function judgmentModelChoices(
  base: readonly ModelChoice[],
  offered: readonly SystemOneEndpointSpec[],
): ModelChoice[] {
  const sectioned = base.some((choice) => choice.section);
  return [
    ...base,
    ...offered.map((endpoint) => (
      sectioned ? { ...endpoint.choice, section: SYSTEM_ONE_SECTION } : endpoint.choice
    )),
  ];
}

/** Which model row reads as selected: the picked System One row, or the stored
 *  chat model. */
export function judgmentSelectedModel(
  picked: SystemOneEndpointSpec | null,
  storedModel: string,
): string {
  return picked ? picked.choice.value : storedModel;
}

/** The effort the field shows. A System One row has no tiers, so it shows none. */
export function judgmentSelectedEffort(
  picked: SystemOneEndpointSpec | null,
  storedEffort: string | null,
): string | null {
  return picked ? null : storedEffort;
}

/** What one pick writes. */
export interface JudgmentPickWrites {
  /** The `judgment_*` value, always written. */
  judgment: string;
  /** The model pair, written only when a chat model was picked. `null` on a
   *  System One pick, which must leave the stored model where it is. */
  selection: ModelSelectionPatch | null;
}

/**
 * Turn one pick into its writes.
 *
 * **A chat pick writes BOTH halves.** Picking a model while the site runs on a
 * System One row must move the backend back too. Otherwise the field shows a
 * model the engine is not using.
 *
 * **A System One pick writes one.** Leaving the model alone is what makes
 * switching back restore the model the user had, rather than a default.
 *
 * The two writes are independent, and deliberately so. `savePreference` never
 * rejects, so neither write can strand the other, and the order is about which
 * one the server should see first.
 */
export function judgmentPickWrites(patch: ModelSelectionPatch): JudgmentPickWrites {
  const endpoint = SYSTEM_ONE_ENDPOINTS.find((e) => e.choice.value === patch.model);
  if (endpoint) return { judgment: endpoint.id, selection: null };
  return { judgment: CHAT, selection: patch };
}

/**
 * Why the engine is not acting on the shown selection, or `null`.
 *
 * One case only: the field shows a System One row whose provider is switched
 * off. The engine then runs the site on its chat model. The field cannot show
 * the chat model instead, because the stored pick is unchanged and turning the
 * provider back on resumes it.
 */
export function judgmentSelectionCaveat(
  picked: SystemOneEndpointSpec | null,
  switchedOff: boolean,
): string | null {
  if (!picked || !switchedOff) return null;
  return `${SYSTEM_ONE_PROVIDERS[picked.provider].label} is switched off in `
    + `${SYSTEM_ONE_PROVIDER_LOCATION}, so this runs on its chat model`;
}

/**
 * The scope URL a Workers AI token is stored under, or `null` for an account
 * id the engine would refuse.
 *
 * The scope carries the account, so one credential holds everything Clef
 * needs. The engine reads it back with `workers_ai_url`, which accepts only a
 * plain alphanumeric id, so this refuses anything else before it is saved.
 */
export function workersAiScope(accountId: string): string | null {
  const id = accountId.trim();
  return /^[A-Za-z0-9]+$/.test(id) ? `${CLOUDFLARE_ACCOUNTS_PREFIX}${id}/ai` : null;
}

/** The account id inside a stored scope URL, or `null`. */
export function workersAiAccount(scope: string | undefined): string | null {
  const match = scope?.trim().replace(/\/+$/, '').match(/^https:\/\/api\.cloudflare\.com\/client\/v4\/accounts\/([A-Za-z0-9]+)\/ai$/);
  return match ? match[1] : null;
}
