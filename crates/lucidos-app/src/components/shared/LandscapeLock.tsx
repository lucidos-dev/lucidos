import { popupImage } from '../../store/store';

export function LandscapeLock() {
  if (popupImage.value !== null) return null;
  return (
    <div class="landscape-lock" role="alert">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="6" y="2" width="12" height="20" rx="2" />
        <path d="M3 14a9 9 0 0 1 9-9" />
        <polyline points="3 9 3 14 8 14" />
      </svg>
      <p>Please rotate your device to portrait</p>
    </div>
  );
}
