/**
 * What Settings, System, Maintenance says about how THIS install takes an
 * update, beyond what its own controls already say.
 *
 * It exists because `updateRoute`'s `guide` lands on this page (ADR 0142), so
 * the page owes an answer to every install shape that arrives here. A browser
 * session on a bundle used to arrive and find the version, a Check button, and
 * no word on where the install happens. Pressing Check re-found the same
 * release and sent them here again.
 */
import type { InstallShape } from '../../api/client/control';

/** The sentence to show, or `null` when the controls already say it. */
export type UpdateGuidance = 'source-checkout' | 'install-in-the-app' | null;

export interface UpdateGuidanceInput {
  /** Has `/health` answered? Until it has, `packaged` is a default, not a fact,
   *  and this page is exactly where a user with a dead engine comes to look. */
  engineAnswered: boolean;
  /** The engine's own `packaged` flag. False means it runs from a checkout. */
  packaged: boolean;
  /** Is there a release on offer to act on? */
  hasOffer: boolean;
  /** Would this session install an offer, if one existed? */
  sessionCanInstall: boolean;
  /** Can this session run a check at all? */
  canCheckHere: boolean;
  /** How the gateway says this install updates, when it has said. */
  install: InstallShape | null;
}

/**
 * Pure, and the whole of the rule. Each `null` names a control on the page that
 * has already answered, so the guidance never repeats one.
 */
export function updateGuidance(input: UpdateGuidanceInput): UpdateGuidance {
  if (!input.engineAnswered) return null;
  if (!input.packaged) return 'source-checkout';
  // A headless install's answer is the installer command below, which the
  // gateway composes from this instance.
  if (input.install === 'installer-rerun') return null;
  // A session that would install an offer is not the one being told where the
  // app is. It IS the app, offer or no offer.
  if (input.sessionCanInstall) return null;
  // An offer this session cannot take, or no way to look for one at all. Either
  // way the control lives in the app on the machine running the engine.
  return input.hasOffer || !input.canCheckHere ? 'install-in-the-app' : null;
}
