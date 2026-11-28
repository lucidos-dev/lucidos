import { HomeIcon } from './icons';

export function ThreadIcon({ home }: { home: boolean }) {
  if (!home) return null;
  return <span class="thread-icon"><HomeIcon /></span>;
}
