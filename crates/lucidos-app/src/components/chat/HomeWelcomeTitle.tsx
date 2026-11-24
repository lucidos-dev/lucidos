import { DesktopThreadTitleBar } from './ThreadView';
import { MobileThreadTitleBar } from '../layout/MobileAppHeader';

export function HomeWelcomeTitle({ threadId }: { threadId: string }) {
  return (
    <div class="home-welcome-title">
      <DesktopThreadTitleBar threadId={threadId} />
      <MobileThreadTitleBar />
    </div>
  );
}
