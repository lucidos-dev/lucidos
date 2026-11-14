/** Connection admission, before thread reads, compose recovery or domain frames.
 * A restore generation is an identity, never an ordered event sequence. */

export type RestoreGeneration = string | null;
export interface GenerationExpectation { generation: RestoreGeneration | undefined }

export interface RestoreGenerationStore<Intent> {
  readGeneration(): Promise<RestoreGeneration | undefined>;
  /** Commit the observation and this page's intent together. Must retain other
   * pages' recovery records and records awaiting review after an earlier restore. */
  preserve(generation: RestoreGeneration, intent: Intent, expected?: GenerationExpectation): Promise<void>;
  observe(generation: RestoreGeneration, expected?: GenerationExpectation): Promise<void>;
}

export interface RestoreGenerationEffects<Intent> {
  /** Includes draining pending local writes; never clears uploads or unsent sends. */
  capture(): Promise<Intent>;
  reload(): void;
}

export type GenerationAdmission = 'ready' | 'reload' | 'stale';

export class RestoreGenerationBarrier<Intent> {
  private epoch = 0;
  private ready = false;
  private reloading = false;
  private generation: RestoreGeneration | undefined;
  private readonly initial: Promise<void>;
  private initialFailure: unknown;
  /** False once a failed page-load read made admission trust a later read. */
  baselineFromPageLoad = true;
  private tail: Promise<unknown> = Promise.resolve();
  private waiters: Array<() => void> = [];

  constructor(
    private readonly store: RestoreGenerationStore<Intent>,
    private readonly effects: RestoreGenerationEffects<Intent>,
  ) {
    // Capture this page's baseline once. Another tab can advance the durable
    // marker later; that must not make this page's old state safe to dispatch.
    this.initial = store.readGeneration().then((generation) => {
      this.generation = generation;
    }).catch((error: unknown) => { this.initialFailure = error; });
  }

  beginConnection(): number {
    this.ready = false;
    return ++this.epoch;
  }

  isReady(epoch: number): boolean {
    return epoch === this.epoch && this.ready && !this.reloading;
  }

  /** Failed persistence leaves callers parked. A later successful hello may
   * release them; no timeout is permission to replay intent. The returned
   * connection lease must still pass isReady before applying an async read. */
  async waitUntilReady(): Promise<number> {
    while (!this.ready || this.reloading) {
      await new Promise<void>((resolve) => { this.waiters.push(resolve); });
    }
    return this.epoch;
  }

  acceptHello(epoch: number, generation: RestoreGeneration): Promise<GenerationAdmission> {
    const result = this.tail.then(() => this.admit(epoch, generation));
    // Preserve ordering while allowing a failed write to retry on reconnect.
    this.tail = result.catch(() => {});
    return result;
  }

  private async admit(epoch: number, generation: RestoreGeneration): Promise<GenerationAdmission> {
    await this.initial;
    if (epoch !== this.epoch) return 'stale';
    if (this.reloading) return 'reload';
    if (this.initialFailure !== undefined) {
      // Opening IndexedDB can fail transiently in a backgrounded WebKit tab.
      this.generation = await this.store.readGeneration();
      this.initialFailure = undefined;
      this.baselineFromPageLoad = false;
    }
    const changed = this.generation !== generation
      && !(this.generation === undefined && generation === null);
    if (changed) {
      const intent = await this.effects.capture();
      if (epoch !== this.epoch) return 'stale';
      await this.store.preserve(generation, intent, { generation: this.generation });
      if (epoch !== this.epoch) return 'stale';
      this.reloading = true;
      this.effects.reload();
      return 'reload';
    }
    await this.store.observe(generation, { generation: this.generation });
    if (epoch !== this.epoch) return 'stale';
    this.generation = generation;
    this.ready = true;
    const waiters = this.waiters;
    this.waiters = [];
    for (const resolve of waiters) resolve();
    return 'ready';
  }
}
