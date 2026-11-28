
import type { BootAction } from './bootSplash';

export const STARTUP_STATUS_POLL_MS = 1_000;

export type StartupCommand = 'startup_status' | 'trust_loopback_ca';

interface StartupReport {
  label: string;
  trustButton: string | null;
}

export interface StartupStatusDeps {
  invoke: (cmd: StartupCommand) => Promise<unknown>;
  setStatus: (text: string) => void;
  setAction: (action: BootAction | null) => void;
  setInterval: (fn: () => void, ms: number) => number;
}

function parseReport(answer: unknown): StartupReport | null {
  if (typeof answer !== 'object' || answer === null) return null;
  const { label, trustButton } = answer as Record<string, unknown>;
  if (typeof label !== 'string' || label.length === 0) return null;
  return { label, trustButton: typeof trustButton === 'string' ? trustButton : null };
}

export function startStartupStatusPolling(deps: StartupStatusDeps): number {
  const press = () => {
    deps.setAction(null);
    void deps.invoke('trust_loopback_ca').then(tick, tick);
  };
  function tick() {
    void deps
      .invoke('startup_status')
      .then((answer) => {
        const report = parseReport(answer);
        if (!report) return;
        deps.setStatus(report.label);
        deps.setAction(report.trustButton ? { label: report.trustButton, onPress: press } : null);
      })
      .catch(() => {});
  }
  tick();
  return deps.setInterval(tick, STARTUP_STATUS_POLL_MS);
}
