/* GENERATED from packages/lucidos-sdk/src/worker/ by sseWorker.build.mjs.
   Do not edit: run `npm run build` in packages/lucidos-sdk. */
"use strict";
(() => {
  // src/eventStream.ts
  function postPong(pongUrl2, notificationId, answer) {
    void fetch(pongUrl2, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ notification_id: notificationId, ...answer }),
      keepalive: true
    }).catch((e) => {
      console.warn("[PresencePong] Failed to POST:", e);
    });
  }

  // src/worker/protocol.ts
  var PONG_COLLECT_MS = 300;
  function aggregatePongAnswers(answers) {
    if (answers.length === 0) return null;
    const active = answers.filter((a) => a.is_active);
    const preferred = active.length > 0 ? active : answers;
    return {
      device_id: answers[0].device_id,
      is_active: active.length > 0,
      event_in_viewport: answers.some((a) => a.event_in_viewport),
      focused_thread_id: preferred.find((a) => a.focused_thread_id !== null)?.focused_thread_id ?? null
    };
  }

  // src/worker/sseWorker.ts
  var clients = /* @__PURE__ */ new Set();
  var source = null;
  var streamUrl = "";
  var pongUrl = "";
  var upstreamOpen = false;
  var reconnectTimer = null;
  var RECONNECT_MS = 3e3;
  function send(client, msg) {
    try {
      client.port.postMessage(msg);
    } catch {
      clients.delete(client);
    }
  }
  function broadcast(msg) {
    for (const client of [...clients]) send(client, msg);
  }
  function closeUpstream() {
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    source?.close();
    source = null;
    upstreamOpen = false;
  }
  function ensureSource() {
    if (source) return;
    const es = new EventSource(streamUrl);
    source = es;
    es.onmessage = (event) => {
      const data = event.data;
      maybeStartPongCollection(data);
      broadcast({ t: "frame", data });
    };
    es.onopen = () => {
      upstreamOpen = true;
      broadcast({ t: "open" });
    };
    es.onerror = () => {
      upstreamOpen = false;
      broadcast({ t: "error" });
      es.close();
      source = null;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null;
        if (clients.size > 0) ensureSource();
      }, RECONNECT_MS);
    };
  }
  var collecting = /* @__PURE__ */ new Map();
  function maybeStartPongCollection(data) {
    if (!data.includes("PresenceCheck")) return;
    let parsed;
    try {
      parsed = JSON.parse(data);
    } catch {
      return;
    }
    if (parsed?.type !== "PresenceCheck") return;
    const id = parsed.data?.notification_id;
    if (typeof id !== "string" || collecting.has(id)) return;
    const expected = [...clients].filter((c) => c.pongs).length;
    if (expected === 0) return;
    const timer = setTimeout(() => settle(id), PONG_COLLECT_MS);
    collecting.set(id, { expected, answers: [], timer });
  }
  function collectPong(notificationId, answer) {
    const open = collecting.get(notificationId);
    if (!open) return;
    open.answers.push(answer);
    if (open.answers.length >= open.expected) settle(notificationId);
  }
  function settle(notificationId) {
    const open = collecting.get(notificationId);
    if (!open) return;
    clearTimeout(open.timer);
    collecting.delete(notificationId);
    const merged = aggregatePongAnswers(open.answers);
    if (merged) postPong(pongUrl, notificationId, merged);
  }
  function attach(port) {
    const client = { port, pongs: false };
    port.onmessage = (event) => {
      const msg = event.data;
      if (!msg || typeof msg !== "object") return;
      if (msg.t === "hello") {
        client.pongs = msg.pongs;
        streamUrl || (streamUrl = msg.streamUrl);
        pongUrl || (pongUrl = msg.pongUrl);
        clients.add(client);
        ensureSource();
        if (upstreamOpen) send(client, { t: "open", lateJoin: true });
        return;
      }
      if (msg.t === "pong") {
        collectPong(msg.notificationId, msg.answer);
        return;
      }
      if (msg.t === "bye") {
        clients.delete(client);
        port.close();
        if (clients.size === 0) closeUpstream();
      }
    };
    port.start();
  }
  self.onconnect = (event) => {
    attach(event.ports[0]);
  };
})();
