import { gapRefreshedVersion } from './eventGap';

/** Moves on `PairingRequestsChanged`: a pairing started waiting for a paired
 *  device's Allow, or one was answered. Settings → Access re-reads its list. */
export const pairingRequestsVersion = gapRefreshedVersion();
