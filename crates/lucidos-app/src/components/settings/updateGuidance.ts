import type { InstallShape } from '../../api/client/control';

export type UpdateGuidance = 'source-checkout' | 'install-in-the-app' | null;

export interface UpdateGuidanceInput {
  engineAnswered: boolean;
  packaged: boolean;
  hasOffer: boolean;
  sessionCanInstall: boolean;
  canCheckHere: boolean;
  install: InstallShape | null;
}

export function updateGuidance(input: UpdateGuidanceInput): UpdateGuidance {
  if (!input.engineAnswered) return null;
  if (!input.packaged) return 'source-checkout';
  if (input.install === 'installer-rerun') return null;
  if (input.sessionCanInstall) return null;
  return input.hasOffer || !input.canCheckHere ? 'install-in-the-app' : null;
}
