import {
  SYSTEM_THEME_MODE_CONFIRM_MS,
  SYSTEM_THEME_MODE_SETTLE_MS,
} from '../../../packages/lucidos-sdk/src/appearance';

/** What a repaint and a loaded runner may add on top of the follower's two
 *  reads. */
const REPAINT_SLACK_MS = 1_700;

/** How long a page on `system` takes to paint a flip the OS announced while
 *  visible: the settle read, then the confirming read, then a repaint. */
export const SYSTEM_THEME_GRACE_MS =
  SYSTEM_THEME_MODE_SETTLE_MS + SYSTEM_THEME_MODE_CONFIRM_MS + REPAINT_SLACK_MS;
