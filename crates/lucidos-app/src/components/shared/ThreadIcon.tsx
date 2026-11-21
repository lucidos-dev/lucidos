import { HomeIcon } from './icons';

/** A thread's icon, drawn before its title in a drawer row and in the title
 *  band. Only the home thread has one. Each surface sizes it in CSS. */
export function ThreadIcon({ home }: { home: boolean }) {
  if (!home) return null;
  return <span class="thread-icon"><HomeIcon /></span>;
}
