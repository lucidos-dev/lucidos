import { useRef, useEffect } from 'preact/hooks';
import { threadChannelFilter } from '../../store/store';
import { CHANNEL_OPTIONS, toggleChannel } from './headerHelpers';

export function ThreadFilterDropdown({ onClose, toggleRef }: { onClose: () => void; toggleRef: { current: HTMLButtonElement | null } }) {
  const ref = useRef<HTMLDivElement>(null);
  const filter = threadChannelFilter.value;

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (toggleRef.current?.contains(e.target as Node)) return;
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    }
    document.addEventListener('click', handleClick);
    return () => document.removeEventListener('click', handleClick);
  }, [onClose]);

  return (
    <div class="thread-filter-dropdown" ref={ref}>
      <div class="thread-filter-title">Show</div>
      {CHANNEL_OPTIONS.map(opt => (
        <label class="thread-filter-option" key={opt.value}>
          <input
            type="checkbox"
            checked={filter.has(opt.value)}
            onChange={() => toggleChannel(opt.value)}
          />
          {opt.label}
        </label>
      ))}
    </div>
  );
}
