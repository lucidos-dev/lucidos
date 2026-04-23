import { ThreadDrawer } from '../drawer/ThreadDrawer';

/** Standalone threads pane for the mobile swipe container. */
export function MobileThreadsPane() {
  return (
    <div class="mobile-threads-pane">
      <ThreadDrawer forceVisible />
    </div>
  );
}
