
export type RestoreGeneration = string | null;
export interface GenerationExpectation { generation: RestoreGeneration | undefined }

export interface RestoreGenerationStore<Intent> {
  readGeneration(): Promise<RestoreGeneration | undefined>;
  preserve(generation: RestoreGeneration, intent: Intent, expected?: GenerationExpectation): Promise<void>;
  observe(generation: RestoreGeneration, expected?: GenerationExpectation): Promise<void>;
}

export interface RestoreGenerationEffects<Intent> {
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
  baselineFromPageLoad = true;
  private tail: Promise<unknown> = Promise.resolve();
  private waiters: Array<() => void> = [];

  constructor(
    private readonly store: RestoreGenerationStore<Intent>,
    private readonly effects: RestoreGenerationEffects<Intent>,
  ) {
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

  async waitUntilReady(): Promise<number> {
    while (!this.ready || this.reloading) {
      await new Promise<void>((resolve) => { this.waiters.push(resolve); });
    }
    return this.epoch;
  }

  acceptHello(epoch: number, generation: RestoreGeneration): Promise<GenerationAdmission> {
    const result = this.tail.then(() => this.admit(epoch, generation));
    this.tail = result.catch(() => {});
    return result;
  }

  private async admit(epoch: number, generation: RestoreGeneration): Promise<GenerationAdmission> {
    await this.initial;
    if (epoch !== this.epoch) return 'stale';
    if (this.reloading) return 'reload';
    if (this.initialFailure !== undefined) {
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
