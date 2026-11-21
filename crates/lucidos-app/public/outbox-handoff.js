// The http half of the outbox hand-off (ADR 0745), served by the gateway to a
// direct client on plain loopback. It reads this origin's outbox, posts it to
// the https import page, and clears this origin only after that page has
// acknowledged every record. In the desktop window it stores the export in the
// native outbox store instead, the one command that origin may call. It reads
// no cookie and calls no API: the one request it makes is the same-origin
// clear. The names below must equal the
// TypeScript ones (`src/handoff/outboxHandoff.test.ts` pins them).
window.lucidosOutboxHandoff = (function () {
  var names = {
    importPath: '/~/outbox-import',
    clearPath: '/~/outbox-handoff/clear',
    messages: {
      ready: 'lucidos-outbox-import-ready',
      export: 'lucidos-outbox-export',
      ack: 'lucidos-outbox-import-ack',
    },
    version: 1,
    nativeStore: 'store_outbox_handoff',
    databases: {
      unsentMessages: { base: 'lucidos-unsent-messages', store: 'messages' },
      pendingUploads: { base: 'lucidos-pending-uploads', store: 'uploads' },
    },
    // Recovered drafts the user has not reviewed, carried with their keys.
    restoreJournal: {
      base: 'lucidos-restore-generation',
      stores: { observations: 'observations', uploadBytes: 'upload-bytes' },
    },
  };

  // The workspace id a database named `<base>` or `<base>:<id>` belongs to:
  // null for the bare base, undefined when the name is not one.
  function workspaceOf(name, base) {
    if (name === base) return null;
    return name.startsWith(base + ':') ? name.slice(base.length + 1) : undefined;
  }

  // The outbox database `name` is, with its store and workspace; else null.
  function outboxDatabase(name) {
    for (var field of Object.keys(names.databases)) {
      var database = names.databases[field];
      var workspaceId = workspaceOf(name, database.base);
      if (workspaceId !== undefined) return { field: field, store: database.store, workspaceId: workspaceId };
    }
    return null;
  }

  // Every record in one store, as `[key, value]` pairs. Never creates a
  // database: one that vanished since the listing aborts its upgrade and
  // reads as empty.
  function readEntries(idb, name, store) {
    return new Promise(function (resolve, reject) {
      var vanished = false;
      var request = idb.open(name);
      request.onupgradeneeded = function () {
        vanished = true;
        request.transaction.abort();
      };
      request.onerror = function () {
        if (vanished) resolve([]);
        else reject(request.error);
      };
      request.onsuccess = function () {
        var db = request.result;
        if (!db.objectStoreNames.contains(store)) {
          db.close();
          resolve([]);
          return;
        }
        var transaction = db.transaction(store, 'readonly');
        var keys = transaction.objectStore(store).getAllKeys();
        var values = transaction.objectStore(store).getAll();
        transaction.oncomplete = function () {
          db.close();
          resolve(keys.result.map(function (key, i) { return [key, values.result[i]]; }));
        };
        transaction.onerror = function () {
          db.close();
          reject(transaction.error);
        };
      };
    });
  }

  async function exportOutbox(idb) {
    if (!idb || typeof idb.databases !== 'function') {
      throw new Error('This browser cannot list the work it saved, so nothing was moved or cleared');
    }
    var workspaces = new Map();
    function workspace(workspaceId) {
      var key = JSON.stringify(workspaceId);
      var found = workspaces.get(key) || { workspaceId: workspaceId, unsentMessages: [], pendingUploads: [] };
      workspaces.set(key, found);
      return found;
    }
    var journal = names.restoreJournal;
    for (var info of await idb.databases()) {
      if (!info.name) continue;
      var found = outboxDatabase(info.name);
      if (found) {
        var entries = await readEntries(idb, info.name, found.store);
        if (entries.length > 0) {
          workspace(found.workspaceId)[found.field] = entries.map(function (entry) { return entry[1]; });
        }
        continue;
      }
      var journalOf = workspaceOf(info.name, journal.base);
      if (journalOf === undefined) continue;
      var observations = await readEntries(idb, info.name, journal.stores.observations);
      if (observations.length === 0) continue;
      workspace(journalOf).restoreJournal = {
        observations: observations,
        uploadBytes: await readEntries(idb, info.name, journal.stores.uploadBytes),
      };
    }
    return { kind: names.messages.export, version: names.version, workspaces: Array.from(workspaces.values()) };
  }

  function base64(buffer) {
    var bytes = new Uint8Array(buffer);
    var binary = '';
    for (var i = 0; i < bytes.length; i += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return btoa(binary);
  }

  // The export as the desktop's native store keeps it: JSON, with every
  // `bytes` field in base64 (`decodeNativeOutbox` reads it).
  function encodeNative(exported) {
    return JSON.stringify(exported, function (key, value) {
      return key === 'bytes' && value instanceof ArrayBuffer ? base64(value) : value;
    });
  }

  // A thrown Error, or the plain string a Tauri command rejects with.
  function reason(error) {
    return error && error.message ? error.message : String(error);
  }

  // Equal to `outboxRecordCount` in `src/handoff/outboxHandoff.ts`.
  function recordCount(exported) {
    return exported.workspaces.reduce(function (sum, workspace) {
      var journal = workspace.restoreJournal;
      return sum + workspace.unsentMessages.length + workspace.pendingUploads.length
        + (journal ? journal.observations.length : 0);
    }, 0);
  }

  function units(count) {
    return count === 1 ? '1 unsent item' : count + ' unsent items';
  }

  // Open the import page, send it the export once it says it is ready, and
  // resolve once it acknowledges every record.
  function handOff(deps, exported, target) {
    return new Promise(function (resolve, reject) {
      var opened = deps.open(target + names.importPath);
      if (!opened) {
        reject(new Error('The browser blocked the new window. Allow pop-ups for this address, then press Continue again'));
        return;
      }
      var expected = recordCount(exported);
      function stop() {
        deps.removeMessageListener(listener);
        stopWatching();
      }
      function listener(event) {
        if (event.source !== opened || event.origin !== target) return;
        var data = event.data || {};
        if (data.kind === names.messages.ready) {
          opened.postMessage(exported, target);
        } else if (data.kind === names.messages.ack) {
          stop();
          if (data.error) reject(new Error(data.error));
          else if (data.imported !== expected) reject(new Error('Only ' + data.imported + ' of ' + expected + ' items arrived'));
          else resolve(expected);
        }
      }
      deps.addMessageListener(listener);
      // A window has no close event, so it is checked: one closed before its
      // answer will never answer, and the user gets Continue back.
      var stopWatching = deps.every(function () {
        if (!opened.closed) return;
        stop();
        reject(new Error('The new window closed before your unsent work arrived'));
      }, 500);
    });
  }

  // The gateway answers with `Clear-Site-Data`, for this http origin only.
  async function clear(deps) {
    var response = await deps.fetch(names.clearPath, { method: 'POST' });
    if (!response.ok) throw new Error('Could not clear the old address (HTTP ' + response.status + ')');
  }

  function render(text, action) {
    var status = document.querySelector('.boot-splash-status');
    if (status) status.textContent = text;
    var previous = document.querySelector('[data-role="outbox-handoff-action"]');
    if (previous) previous.remove();
    if (!action || !status) return;
    var button = document.createElement('button');
    button.type = 'button';
    button.className = 'action-btn';
    button.dataset.role = 'outbox-handoff-action';
    button.textContent = action.label;
    button.addEventListener('click', action.onPress, { once: true });
    status.after(button);
  }

  function defaults() {
    return {
      indexedDB: window.indexedDB,
      location: window.location,
      open: function (url) { return window.open(url); },
      every: function (check, ms) {
        var id = window.setInterval(check, ms);
        return function () { window.clearInterval(id); };
      },
      fetch: function (url, init) { return window.fetch(url, init); },
      invoke: window.__TAURI_INTERNALS__
        ? function (cmd, args) { return window.__TAURI_INTERNALS__.invoke(cmd, args); }
        : null,
      addMessageListener: function (listener) { window.addEventListener('message', listener); },
      removeMessageListener: function (listener) { window.removeEventListener('message', listener); },
      render: render,
    };
  }

  async function start(overrides) {
    var deps = Object.assign(defaults(), overrides);
    var target = 'https://localhost:' + deps.location.port;
    var next = target + deps.location.pathname + deps.location.search + deps.location.hash;
    var exported;
    function moveOn(text) {
      deps.render(text, { label: 'Continue', onPress: function () { deps.location.replace(next); } });
    }
    if (deps.invoke) {
      // The desktop window has no second window to post to. So the export goes
      // to the native store, and the https page takes it from there. The store
      // is called even with nothing to keep: that call is what marks the
      // hand-off done, so the window skips it on every later launch.
      try {
        exported = await exportOutbox(deps.indexedDB);
        var payload = recordCount(exported) > 0 ? encodeNative(exported) : null;
        await deps.invoke(names.nativeStore, { payload: payload });
        await clear(deps);
      } catch (error) {
        moveOn(reason(error) + '. Nothing unsent was lost.');
        return;
      }
      deps.location.replace(next);
      return;
    }
    try {
      exported = await exportOutbox(deps.indexedDB);
    } catch (error) {
      moveOn(error.message + '.');
      return;
    }
    var count = recordCount(exported);
    if (count === 0) {
      try {
        await clear(deps);
      } catch (error) {
        moveOn(error.message + '. Nothing unsent was lost.');
        return;
      }
      deps.location.replace(next);
      return;
    }
    function offer(text) {
      deps.render(text, {
        label: 'Continue',
        onPress: function () {
          deps.render('Finish in the new window…', null);
          handOff(deps, exported, target)
            .then(function () { return clear(deps); })
            .then(function () {
              deps.render(units(count) + ' moved to the secure address. Carry on in the new window.', null);
            }, function (error) {
              offer(error.message + '. Your unsent work is still here.');
            });
        },
      });
    }
    offer(units(count) + ' from before Lucidos moved to a secure address are waiting. Continue to move them.');
  }

  return { names: names, exportOutbox: exportOutbox, start: start };
})();
