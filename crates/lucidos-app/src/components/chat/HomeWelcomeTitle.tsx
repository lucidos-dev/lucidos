import { DesktopThreadTitleBar } from './ThreadView';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';

/** Home's title over an empty Home's compose layout (ADR 0411).
 *
 *  The thread view draws the title above its transcript. An empty Home showing
 *  the welcome has no transcript: the welcome and the prompt sit centred, as
 *  on a first run. So the title rides at the pane's top instead, outside that
 *  centred group, in the same row each layout's thread view uses. */
export function HomeWelcomeTitle({ threadId }: { threadId: string }) {
  return (
    <div class="home-welcome-title">
      <DesktopThreadTitleBar threadId={threadId} />
      <MobileThreadTitleBar />
    </div>
  );
}
