export function SearchIcon({ className, size }: { className?: string; size?: string } = {}) {
  return (
    <svg class={className} {...(size ? { width: size, height: size } : {})} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="7" cy="7" r="4.5" />
      <path d="M10.5 10.5L14 14" />
    </svg>
  );
}

