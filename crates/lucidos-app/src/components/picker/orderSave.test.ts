import { describe, expect, it, vi } from 'vitest';
import { makeOrderSaver, type OrderSaveDeps } from './orderSave';

/** A gateway whose saves answer only when the test says so. */
function harness() {
  const drawn: (readonly string[])[] = [];
  const sent: (readonly string[])[] = [];
  const answers: { resolve(): void; reject(e: Error): void }[] = [];
  const deps: OrderSaveDeps = {
    draw: (order) => drawn.push(order),
    save: (order) => {
      sent.push(order);
      return new Promise<void>((resolve, reject) => answers.push({ resolve, reject }));
    },
    report: vi.fn(),
  };
  return { drawn, sent, answers, deps, saver: makeOrderSaver(deps) };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('makeOrderSaver', () => {
  it('draws the new order before the save answers, and keeps it', async () => {
    const { drawn, answers, deps, saver } = harness();
    saver.stored(['a', 'b'], saver.readStarted());
    const done = saver.reorder(['b', 'a']);
    expect(drawn).toEqual([['a', 'b'], ['b', 'a']]);
    answers[0].resolve();
    await done;
    expect(drawn).toEqual([['a', 'b'], ['b', 'a']]);
    expect(deps.report).not.toHaveBeenCalled();
  });

  it('returns to the stored order and says why when the save is refused', async () => {
    const { drawn, answers, deps, saver } = harness();
    saver.stored(['a', 'b'], saver.readStarted());
    const done = saver.reorder(['b', 'a']);
    answers[0].reject(new Error('gateway said no'));
    await done;
    expect(drawn[drawn.length - 1]).toEqual(['a', 'b']);
    expect(deps.report).toHaveBeenCalledWith('Could not save the workspace order: gateway said no');
  });

  it('sends one save at a time, and only the newest waiting order', async () => {
    const { sent, answers, saver } = harness();
    const done = saver.reorder(['b', 'a', 'c']);
    saver.reorder(['c', 'b', 'a']);
    saver.reorder(['a', 'c', 'b']);
    expect(sent).toEqual([['b', 'a', 'c']]);
    answers[0].resolve();
    await settle();
    expect(sent).toEqual([['b', 'a', 'c'], ['a', 'c', 'b']]);
    answers[1].resolve();
    await done;
  });

  it('stays silent when a refused save already has a newer order behind it', async () => {
    const { drawn, answers, deps, saver } = harness();
    const done = saver.reorder(['b', 'a']);
    saver.reorder(['a', 'b']);
    answers[0].reject(new Error('gateway said no'));
    await settle();
    expect(deps.report).not.toHaveBeenCalled();
    answers[1].resolve();
    await done;
    expect(drawn[drawn.length - 1]).toEqual(['a', 'b']);
  });

  it('ignores a read that began before a save, or ran during one', async () => {
    const { drawn, answers, saver } = harness();
    const before = saver.readStarted();
    const done = saver.reorder(['b', 'a']);
    saver.stored(['a', 'b'], before);
    saver.stored(['a', 'b'], saver.readStarted());
    answers[0].resolve();
    await done;
    saver.stored(['a', 'b'], before);
    expect(drawn).toEqual([['b', 'a']]);
    saver.stored(['x'], saver.readStarted());
    expect(drawn[drawn.length - 1]).toEqual(['x']);
  });
});
