import { useState } from 'preact/hooks';
import type { ReleaseNotice, ReleaseNoticeAnswer, ReleaseNoticeItem } from '../../api/client';
import { releaseNoticeSubmitting, submitReleaseNotice } from '../../store/actions/releaseNotices';
import { Disclosure } from './Disclosure';
import { ChevronDownIcon, ChevronRightIcon } from './icons';
import { ReleaseNoticeBody } from './ReleaseNoticeBody';
import { SkBlock, SkText, useSkeleton } from './Skeleton';

export function releaseNoticeHeading(since: string, itemCount: number): string {
  return `Lucidos ${since}: ${itemCount} ${itemCount === 1 ? 'thing' : 'things'} for you`;
}

function NoticeItem({ item, onNavigate }: { item?: ReleaseNoticeItem; onNavigate?: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <li class="release-notice-item">
      <SkText as="div" class="release-notice-item-title" w="12rem">{item?.title}</SkText>
      <p class="release-notice-summary"><SkText w="75%">{item?.summary}</SkText></p>
      <SkBlock w="6rem" h="1.5rem">
        {item && (
          <button
            type="button"
            class="release-notice-read"
            aria-label={`${open ? 'Read less' : 'Read more'} about ${item.title}`}
            aria-expanded={open}
            onClick={() => setOpen(!open)}
          >
            {open ? 'Read less' : 'Read more'}
            {open ? <ChevronDownIcon size="0.875rem" /> : <ChevronRightIcon size="0.875rem" />}
          </button>
        )}
      </SkBlock>
      {item && (
        <Disclosure open={open}>
          <ReleaseNoticeBody body={item.body} onNavigate={onNavigate} />
        </Disclosure>
      )}
    </li>
  );
}

export type ReleaseNoticeItemFilter = 'owed' | 'answered';

export function ReleaseNoticeCard({ release, showHeading = true, show, onNavigate }: {
  release?: ReleaseNotice;
  showHeading?: boolean;
  show: ReleaseNoticeItemFilter;
  onNavigate?: () => void;
}) {
  const sk = useSkeleton();
  const visible = (release?.items ?? []).filter((item) => item.resolved === (show === 'answered'));
  return (
    <section class="release-notice-card" aria-label={release && `Lucidos ${release.since}`}>
      {showHeading && (
        <h3><SkText w="14rem">{release && releaseNoticeHeading(release.since, visible.length)}</SkText></h3>
      )}
      <ul class="release-notice-items">
        {sk ? <NoticeItem /> : visible.map((item) => (
          <NoticeItem key={item.id} item={item} onNavigate={onNavigate} />
        ))}
      </ul>
      {release?.resolved && release.action_prompt && (
        <div class="release-notice-actions">
          <ReleaseNoticeActions answer={null} prompt={release.action_prompt} />
        </div>
      )}
    </section>
  );
}

export function ReleaseNoticeActions({ answer, prompt }: {
  answer: ReleaseNoticeAnswer | null;
  prompt: string | null;
}) {
  const busy = releaseNoticeSubmitting.value;
  return (
    <>
      {answer && (
        <button type="button" class="action-btn action-btn-secondary" disabled={busy}
          onClick={() => void submitReleaseNotice(answer)}>
          Got it
        </button>
      )}
      {prompt && (
        <button type="button" class="action-btn" disabled={busy}
          onClick={() => void submitReleaseNotice(answer, prompt)}>
          Check my workspace
        </button>
      )}
    </>
  );
}
