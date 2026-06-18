import { useEffect, useState } from 'preact/hooks';
import { LucidosMark } from './LucidosMark';

// One-time boot splash: a full-screen brand-gradient wash with the white mark
// playing its reveal once, shown on the first page load of a browser session
// and suppressed on every in-session reload/HMR thereafter (sessionStorage).
// Purely visual + time-based — it never blocks on app readiness; the app shell
// loads behind it and the splash fades out on its own.

const SESSION_KEY = 'lucidos-boot-splash-shown';
const REVEAL_MS = 1200; // matches the mark reveal in components.css
const HOLD_MS = 250; // brief hold on the finished mark
const FADE_MS = 450; // matches .boot-splash-leaving in components.css

type Phase = 'visible' | 'leaving' | 'done';

export function BootSplash() {
  const [phase, setPhase] = useState<Phase>(() => {
    try {
      if (sessionStorage.getItem(SESSION_KEY)) return 'done';
      sessionStorage.setItem(SESSION_KEY, '1');
    } catch {
      // sessionStorage unavailable (private mode / disabled): show this load and
      // simply re-show next load — there's no persistence to remember it by.
    }
    return 'visible';
  });

  useEffect(() => {
    if (phase !== 'visible') return;
    const t = setTimeout(() => setPhase('leaving'), REVEAL_MS + HOLD_MS);
    return () => clearTimeout(t);
  }, [phase]);

  useEffect(() => {
    if (phase !== 'leaving') return;
    const t = setTimeout(() => setPhase('done'), FADE_MS);
    return () => clearTimeout(t);
  }, [phase]);

  if (phase === 'done') return null;
  return (
    <div class={`boot-splash${phase === 'leaving' ? ' boot-splash-leaving' : ''}`} aria-hidden="true">
      <LucidosMark background={false} animated size="min(46vmin, 15rem)" />
    </div>
  );
}
