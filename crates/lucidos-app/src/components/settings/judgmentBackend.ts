import type { CredentialInfo, Loadable } from '../../store/types';
import {
  PROVIDER_ENABLED_TYPESAFE_KEY,
  switchValueReadsAsOff,
  type BackgroundModelKey,
  type BackgroundReasoningKey,
} from '../../store/actions/preferences';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice } from '../../store/modelSelection';
import { findProviderCredential } from './providerCredential';

/**
 * Which backend answers a classification, decided in the site's model control.
 *
 * TypeSafe (Jev) is a row in the model step, beside the chat models. That is
 * the whole shape of this module: what the field shows, which rows it offers,
 * and what a pick writes. It replaces a separate switch, which was the one
 * provider on the page you configured twice.
 *
 * Mirrors `llm::judgment::select` in the engine, which reads the same two words
 * out of the same preference. The two must agree, or the field shows a backend
 * the engine does not use. See ADR 0220.
 */

/** The credential service holding the key. Matches the engine's constant. */
export const TYPESAFE_CREDENTIAL_SERVICE = 'typesafe';

/** The launch-environment fallback, named in the UI because the page cannot
 *  see whether it is set. */
export const TYPESAFE_API_KEY_ENV = 'TYPESAFE_API_KEY';

/** The only value that opts a site in. */
export const JEV = 'jev';

/** The default, and what every other value resolves to. */
export const CHAT = 'chat';

export type JudgmentSite = 'command-guard' | 'query-classification';

/** Every site, in the order the key block lists them. */
export const JUDGMENT_SITES: readonly JudgmentSite[] = ['command-guard', 'query-classification'];

export const JUDGMENT_PREFERENCE_KEY: Record<JudgmentSite, string> = {
  'command-guard': 'judgment_command_guard',
  'query-classification': 'judgment_query_classification',
};

/** The model half of the site's *model selection*, used while it runs on chat.
 *  Left alone by a pick of Jev, so switching back restores the same model. */
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

/** Where its control is, for the key block to point at. */
export const JUDGMENT_SITE_LOCATION: Record<JudgmentSite, string> = {
  'command-guard': 'Permissions → Command safety',
  'query-classification': 'Models → Background tasks',
};

/** Where the provider's own row is, for a site row to point back at. */
export const TYPESAFE_PROVIDER_LOCATION = 'Models → Providers';

/**
 * The value the Jev row carries in the model step.
 *
 * Not a registry id and never sent anywhere: the pick handler reads it and
 * writes the `judgment_*` preference instead. It is deliberately unlike every
 * real model id, so a curated background model can never collide with it.
 */
export const JEV_MODEL_VALUE = 'typesafe-jev';

/** How the Jev row reads, the same name the provider row wears. */
export const JEV_MODEL_LABEL = 'TypeSafe (Jev)';

/** The Jev row itself. No reasoning tiers, so the picker settles in one step,
 *  exactly as an image model does. */
export const JEV_MODEL_CHOICE: ModelChoice = {
  value: JEV_MODEL_VALUE,
  label: JEV_MODEL_LABEL,
  description: 'Typed judgment rather than a written answer',
  reasoningEfforts: [],
};

/**
 * Whether a stored preference value opts the site in.
 *
 * Forgiving of case and surrounding space, strict about everything else, which
 * is exactly what the engine's `wants_jev` does. An unrecognized word is not a
 * guess to resolve, so it reads as `chat`.
 */
export function wantsJev(value: string | undefined | null): boolean {
  return value?.trim().toLowerCase() === JEV;
}

/** Whether a key is stored for Jev to use. */
export function typeSafeKeyStored(credentials: Loadable<CredentialInfo[]>): boolean {
  return !!findProviderCredential(credentials, TYPESAFE_CREDENTIAL_SERVICE);
}

/**
 * Whether the master switch has been turned off.
 *
 * Absent means on, so this is false both for a workspace that never touched the
 * switch and for one still loading. What counts as off is the engine's own
 * vocabulary, read through `switchValueReadsAsOff`, for the reason `wantsJev`
 * mirrors `wants_jev`: a value the two sides spell differently is a backend the
 * engine does not run.
 */
export function typeSafeSwitchedOff(prefs: Loadable<Record<string, string>>): boolean {
  return prefs.status === 'loaded'
    && switchValueReadsAsOff(prefs.data[PROVIDER_ENABLED_TYPESAFE_KEY]);
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

/** The sites running on Jev, for the key block's status line. */
export function jevSitesOn(prefs: Loadable<Record<string, string>>): JudgmentSite[] {
  return JUDGMENT_SITES.filter((site) => wantsJev(storedJudgmentValue(prefs, site)));
}

/** What the agent's own route to Jev is called on screen. */
export const JUDGE_TOOL_LABEL = 'The agent’s judge tool';

/**
 * Everything using Jev right now, for the key block's status line.
 *
 * **The judge tool is listed on a stored key alone, because it has no control.**
 * A key plus the master switch on IS its whole condition (ADR 0223), where a
 * site needs Jev picked in its model control. The row draws only inside the open
 * fold, which a switched-off provider never is, so the key is the one thing
 * left to check here.
 */
export function jevConsumers(
  prefs: Loadable<Record<string, string>>,
  keyStored: boolean,
): string[] {
  const sites = jevSitesOn(prefs).map((site) => JUDGMENT_SITE_LABEL[site]);
  return keyStored ? [JUDGE_TOOL_LABEL, ...sites] : sites;
}

/**
 * Whether the Jev row appears in this site's model step.
 *
 * **Unconfigured means absent**, which is how `chatModelOptions` treats a model
 * whose provider holds no key. Offering it would be offering a backend that
 * cannot answer.
 *
 * **A site already on Jev always sees the row.** The engine also reads
 * `TYPESAFE_API_KEY` from the launch environment, which this page cannot see.
 * Drop the row there and the field renders a selection with no way back. That
 * is the one state a picker must never reach, and it is also why the row
 * survives the master switch being off.
 */
export function jevRowOffered(args: {
  onJev: boolean;
  keyStored: boolean;
  typeSafeOff: boolean;
}): boolean {
  return args.onJev || (args.keyStored && !args.typeSafeOff);
}

/** The site's model rows: the background models, and Jev when it is offered. */
export function judgmentModelChoices(
  base: readonly ModelChoice[],
  jevOffered: boolean,
): ModelChoice[] {
  return jevOffered ? [...base, JEV_MODEL_CHOICE] : [...base];
}

/** Which model row reads as selected: the Jev one, or the stored chat model. */
export function judgmentSelectedModel(onJev: boolean, storedModel: string): string {
  return onJev ? JEV_MODEL_VALUE : storedModel;
}

/** The effort the field shows. Jev has no tiers, so it shows none. */
export function judgmentSelectedEffort(onJev: boolean, storedEffort: string): string | null {
  return onJev ? null : storedEffort;
}

/** What one pick writes. */
export interface JudgmentPickWrites {
  /** The `judgment_*` value, always written. */
  judgment: string;
  /** The model pair, written only when a chat model was picked. `null` on a
   *  pick of Jev, which must leave the stored model where it is. */
  selection: ModelSelectionPatch | null;
}

/**
 * Turn one pick into its writes.
 *
 * **A chat pick writes BOTH halves.** Picking a model while the site runs on
 * Jev must move the backend back too. Otherwise the field shows a model the
 * engine is not using.
 *
 * **A Jev pick writes one.** Leaving the model alone is what makes switching
 * back restore the model the user had, rather than a default.
 *
 * The two writes are independent, and deliberately so. `savePreference` never
 * rejects: it applies the value locally, then toasts a refusal or parks a
 * transient failure for the resume flush. So neither write can strand the
 * other, and the order is about which one the server should see first.
 */
export function judgmentPickWrites(patch: ModelSelectionPatch): JudgmentPickWrites {
  if (patch.model === JEV_MODEL_VALUE) return { judgment: JEV, selection: null };
  return { judgment: CHAT, selection: patch };
}

/**
 * Why the engine is not acting on the shown selection, or `null`.
 *
 * One case only: the field says Jev while the master switch is off. There
 * `jev_for` returns nothing and the site runs its chat path. The field cannot
 * show the chat model instead, because the stored pick is still Jev and turning
 * the provider back on resumes it.
 */
export function judgmentSelectionCaveat(onJev: boolean, typeSafeOff: boolean): string | null {
  if (!onJev || !typeSafeOff) return null;
  return `TypeSafe is switched off in ${TYPESAFE_PROVIDER_LOCATION}, so this runs on its chat model`;
}
