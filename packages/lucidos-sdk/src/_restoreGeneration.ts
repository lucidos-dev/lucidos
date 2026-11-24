export class RestoreGenerationFence {
  private generation: string | null = null;
  private observed = false;
  private closed = false;

  observe(value: unknown): void {
    if (value !== null && (typeof value !== 'string'
      || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value))) {
      this.closed = true;
      return;
    }
    const next = typeof value === 'string' ? value.toLowerCase() : null;
    if (this.observed && next !== this.generation) this.closed = true;
    if (!this.observed) this.generation = next;
    this.observed = true;
  }

  capture(): { header: string; assertCurrent(): void } {
    this.observed = true;
    const generation = this.generation;
    return {
      header: generation ?? 'none',
      assertCurrent: () => {
        if (this.closed || generation !== this.generation) {
          throw new Error('Workspace restored; reload and review intent before submitting again.');
        }
      },
    };
  }
}

export const directRestoreGeneration = new RestoreGenerationFence();

export interface DirectRequestGuard {
  readonly header: string;
  assertCurrent(): void;
  wait?(): Promise<void>;
}
let hostCapture: (() => DirectRequestGuard) | undefined;
export function installDirectRequestAdmission(capture: () => DirectRequestGuard): void {
  hostCapture = capture;
}
export function captureDirectRequest(): DirectRequestGuard {
  return hostCapture ? hostCapture() : directRestoreGeneration.capture();
}
