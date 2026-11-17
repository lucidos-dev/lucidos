import { expect, it } from 'vitest';
import { createInteractiveDelivery, type DeliveryView } from './interactiveDelivery';
import { emptyDeliveryJournal, type InteractiveDeliveryBackend, type InteractiveRequest } from './interactiveDeliveryRecords';

const request = (id = 'request'): InteractiveRequest => ({
  id, kind: 'navigation', payload: { target: 'app', id: 'example' }, preference: { threadId: 'source' },
});

function shared() {
  let journal = emptyDeliveryJournal();
  let tail = Promise.resolve();
  const live = new Set<string>();
  let failWrite = false;
  let ownersUnknown = false;
  let afterClaim: (() => void) | undefined;
  const backend: InteractiveDeliveryBackend = {
    async update(change) {
      const next = structuredClone(journal);
      change(next);
      if (failWrite) { failWrite = false; throw new Error('storage unavailable'); }
      journal = next;
      const result = structuredClone(journal);
      if (afterClaim && Object.values(journal.requests).some((record) => record.delivery.status === 'claimed')) {
        const run = afterClaim;
        afterClaim = undefined;
        run();
      }
      return result;
    },
  };
  const accepted: string[] = [];
  const uncertain: string[] = [];
  const notifications: string[] = [];
  function page(id: string, thread: string, shown = true) {
    live.add(id);
    let accept = true;
    let afterPresentation: (() => void) | undefined;
    const view: DeliveryView = { state: {
      id, nativeShown: shown, focusedThreadId: thread, appId: null, authorizing: null, activeAt: 0,
    }, previous: null, changedAt: 0 };
    const coordinator = createInteractiveDelivery({
      ownerId: id, backend,
      exclusive(body) {
        const result = tail.then(body);
        tail = result.then(() => {}, () => {});
        return result;
      },
      liveOwners: async () => ownersUnknown ? null : new Set(live),
      readSelf: () => structuredClone(view),
      present: (r) => {
        if (accept) accepted.push(`${id}:${r.id}`);
        afterPresentation?.();
        return accept;
      },
      changed: () => { notifications.push(id); },
      uncertain: (r) => { uncertain.push(`${id}:${r.id}`); },
      now: () => 1000,
    });
    return { ...coordinator, view, decline: () => { accept = false; }, allow: () => { accept = true; }, afterPresentation: (run: () => void) => { afterPresentation = run; } };
  }
  return { page, live, accepted, uncertain, notifications, afterClaim: (run: () => void) => { afterClaim = run; }, unknownOwners: () => { ownersUnknown = true; }, read: () => structuredClone(journal), failNext: () => { failWrite = true; } };
}

it('serializes two independent runtimes through hide/show gaps without a lost request or duplicate', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  const other = world.page('other', 'elsewhere');
  await Promise.all([owner.publish(), other.publish()]);
  owner.view.state.nativeShown = false;
  await Promise.all([owner.publish(), owner.submit(request()), other.submit(request())]);
  await other.reconsider();
  expect(world.accepted).toEqual(['other:request']);
  owner.view.state.nativeShown = true;
  await Promise.all([owner.publish(), owner.submit(request('shown')), other.submit(request('shown'))]);
  await owner.reconsider();
  expect(world.accepted).toEqual(['other:request', 'owner:shown']);
});

it('retains a rejected nomination and retries another participant without reconnect', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  const other = world.page('other', 'elsewhere');
  await Promise.all([owner.publish(), other.publish()]);
  owner.decline();
  await owner.submit(request());
  expect(world.read().requests.request.delivery.status).toBe('pending');
  await other.reconsider();
  expect(world.accepted).toEqual(['other:request']);
});

it('does not announce unchanged reconsideration, avoiding notification ping pong', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.publish();
  world.notifications.length = 0;
  await owner.reconsider();
  expect(world.notifications).toEqual([]);
});

it('recovers an orphaned claim as uncertain and requires explicit retry', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.publish();
  owner.afterPresentation(world.failNext);
  await expect(owner.submit(request())).rejects.toThrow('storage unavailable');
  expect(world.accepted).toEqual(['owner:request']);
  world.live.delete('owner');
  const reopened = world.page('reopened', 'source');
  await reopened.publish();
  expect(world.uncertain).toEqual(['reopened:request']);
  expect(world.accepted).toHaveLength(1);
  await reopened.retry('request');
  expect(world.accepted).toEqual(['owner:request', 'reopened:request']);
});

it('retains pending work on write failure and preserves all-hidden fallback', async () => {
  const world = shared();
  const owner = world.page('owner', 'source', false);
  await owner.publish();
  world.failNext();
  await expect(owner.submit(request())).rejects.toThrow();
  expect(world.accepted).toEqual([]);
  await owner.submit(request());
  expect(world.accepted).toEqual(['owner:request']);
});

it('keeps every rejection until a participant publishes a new revision', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  const other = world.page('other', 'elsewhere');
  await Promise.all([owner.publish(), other.publish()]);
  owner.decline();
  other.decline();
  await owner.submit(request());
  await other.reconsider();
  world.notifications.length = 0;
  await Promise.all([owner.reconsider(), other.reconsider()]);
  expect(world.notifications).toEqual([]);
  expect(world.read().requests.request.delivery.status).toBe('pending');
});

it('never automatically repeats an uncertain presentation and ignores stale Retry clicks', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.publish();
  owner.afterPresentation(world.failNext);
  await expect(owner.submit(request())).rejects.toThrow();
  owner.afterPresentation(() => {});
  await owner.reconsider();
  await owner.submit(request());
  expect(world.accepted).toHaveLength(1);
  await owner.retry('request');
  await owner.retry('request');
  expect(world.accepted).toHaveLength(2);
});

it('uses current eligibility before previous-view grace and retains browser policy', async () => {
  const world = shared();
  const owner = world.page('owner', 'elsewhere', false);
  owner.view.previous = { ...owner.view.state, focusedThreadId: 'source' };
  owner.view.changedAt = 999;
  const browser = world.page('browser', 'elsewhere');
  delete browser.view.state.nativeShown;
  await Promise.all([owner.publish(), browser.publish()]);
  await Promise.all([owner.submit(request()), browser.submit(request())]);
  expect(world.accepted).toEqual(['browser:request']);
});

it('fails visibly through its caller when ownership cannot be inspected', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  world.unknownOwners();
  await expect(owner.submit(request())).rejects.toThrow('Web Locks');
  expect(world.accepted).toEqual([]);
});

it('adopts pending work after owner death and isolates independent journals', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.publish();
  owner.decline();
  await owner.submit(request());
  world.live.delete('owner');
  const reopened = world.page('reopened', 'source');
  await reopened.publish();
  expect(world.accepted).toEqual(['reopened:request']);
  const isolated = shared();
  await isolated.page('separate-workspace', 'source').publish();
  expect(isolated.accepted).toEqual([]);
});

it('bounds completed receipts without evicting an unresolved form', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.publish();
  await owner.submit({ ...request('form'), kind: 'form' });
  for (let index = 0; index < 515; index++) await owner.submit(request(`completed-${index}`));
  expect(Object.keys(world.read().requests)).toHaveLength(513);
  await owner.submit({ ...request('form'), kind: 'form' });
  expect(world.accepted.filter((id) => id === 'owner:form')).toHaveLength(1);
  await owner.resolve('form');
  expect(Object.keys(world.read().requests)).toHaveLength(512);
}, 20_000);

it('revalidates a nomination after a concurrent local hide and preserves pending work', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  const other = world.page('other', 'elsewhere');
  await Promise.all([owner.publish(), other.publish()]);
  world.afterClaim(() => { owner.view.state.nativeShown = false; });
  await owner.submit(request());
  expect(world.accepted).toEqual([]);
  expect(world.read().requests.request.delivery.status).toBe('pending');
  await other.reconsider();
  expect(world.accepted).toEqual(['other:request']);
});

it('keeps an invalidated all-hidden nomination pending until its state update is processed', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.publish();
  world.afterClaim(() => { owner.view.state.nativeShown = false; });
  await owner.submit(request());
  expect(world.read().requests.request.delivery.status).toBe('pending');
  await owner.publish();
  expect(world.accepted).toEqual(['owner:request']);
});

it('keeps a resolution that arrives before insertion as a tombstone', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.resolve('form:late');
  await owner.submit({ ...request('form:late'), kind: 'form' });
  expect(world.accepted).toEqual([]);
  expect(world.read().requests['form:late'].delivery.status).toBe('resolved');
});

it('snapshots unresolved forms for offline resolution reconciliation', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  await owner.submit({ ...request('form:open'), kind: 'form' });
  await owner.submit(request('navigation'));
  await owner.resolve('form:resolved');
  expect(await owner.formIds()).toEqual(['open']);
  await owner.resolve('form:open');
  expect(await owner.formIds()).toEqual([]);
});

it('resolves an offline batch before reconsidering any of its pending forms', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  owner.decline();
  await owner.submit({ ...request('form:first'), kind: 'form' });
  await owner.submit({ ...request('form:second'), kind: 'form' });
  owner.allow();
  owner.view.state.activeAt = 1;
  await owner.reconcileForms(['form:first', 'form:second'], [], ['form:first', 'form:second']);
  expect(world.accepted).toEqual([]);
  expect(await owner.formIds()).toEqual([]);
});

it('suspends archived forms across owners and restores them without losing their receipt state', async () => {
  const world = shared();
  const owner = world.page('owner', 'source');
  owner.decline();
  await owner.submit({ ...request('form:archived'), kind: 'form' });
  await owner.reconcileForms(['form:archived'], [], []);
  const reopened = world.page('reopened', 'source');
  world.live.delete('owner');
  await reopened.publish();
  await reopened.submit({ ...request('form:archived'), kind: 'form' });
  expect(world.accepted).toEqual([]);
  expect(await reopened.formIds()).toEqual(['archived']);
  await reopened.reconcileForms(['form:archived'], ['form:archived'], []);
  expect(world.accepted).toEqual(['reopened:form:archived']);
  await reopened.reconcileForms(['form:archived'], [], []);
  await reopened.reconcileForms(['form:archived'], ['form:archived'], []);
  expect(world.accepted).toHaveLength(1);
});
