// Copper addon bridge, v1. CANONICAL COPY — every platform injects these exact bytes.
// Canonical path: copper-linux crates/copper-core/src/addons/bridge.js
// Mac copy:       Copper Sources/Search/Fork/Addons/Resources/bridge.js (verbatim)
// Spec: copper-addons CONTRACT.md §5. Injected at document start, main frame only,
// into the slot's own web view, before any addon script runs.
(function () {
  "use strict";
  if (window.copper && window.copper.bridge === 1) return;

  var BRIDGE = 1;
  var nextId = 1;
  var pending = Object.create(null);
  var listeners = Object.create(null);

  // How a message leaves the page. A platform may define
  // window.__copperAddonTransport(string) before this script runs;
  // otherwise WebKit (Mac, Linux) or WebView2 (Windows) is used.
  function transport() {
    if (typeof window.__copperAddonTransport === "function") return window.__copperAddonTransport;
    var w = window.webkit;
    if (w && w.messageHandlers && w.messageHandlers.copperAddon) {
      return function (s) { w.messageHandlers.copperAddon.postMessage(s); };
    }
    var c = window.chrome;
    if (c && c.webview && typeof c.webview.postMessage === "function") {
      return function (s) { c.webview.postMessage(s); };
    }
    return null;
  }

  function CopperError(code, message) {
    var e = new Error(message || code);
    e.name = "CopperError";
    e.code = code;
    return e;
  }

  function call(method, params) {
    return new Promise(function (resolve, reject) {
      var post = transport();
      if (!post) { reject(CopperError("unavailable", "Not running inside Copper")); return; }
      var id = nextId++;
      pending[id] = { resolve: resolve, reject: reject };
      try {
        post(JSON.stringify({ v: BRIDGE, id: id, method: method, params: params === undefined ? {} : params }));
      } catch (err) {
        delete pending[id];
        reject(CopperError("internal", String(err && err.message || err)));
      }
    });
  }

  // Native → page. Not part of the public API.
  var internal = {
    reply: function (id, ok, payload) {
      var p = pending[id];
      if (!p) return;
      delete pending[id];
      if (ok) p.resolve(payload);
      else p.reject(CopperError((payload && payload.code) || "internal", payload && payload.message));
    },
    emit: function (name, payload) {
      var set = listeners[name];
      if (!set) return;
      set.slice().forEach(function (fn) {
        try { fn(payload); } catch (err) { setTimeout(function () { throw err; }); }
      });
    }
  };
  Object.defineProperty(window, "__copperAddon", { value: internal, writable: false, configurable: false });

  var copper = {
    bridge: BRIDGE,
    info: function () { return call("info"); },
    identity: function () { return call("identity"); },
    settings: function () { return call("settings"); },
    storage: {
      get: function (key) { return call("storage.get", { key: key }); },
      set: function (key, value) { return call("storage.set", { key: key, value: value === undefined ? null : value }); },
      remove: function (key) { return call("storage.remove", { key: key }); },
      keys: function () { return call("storage.keys"); }
    },
    fetch: function (url, init) {
      init = init || {};
      return call("fetch", {
        url: String(url),
        method: init.method || "GET",
        headers: init.headers || {},
        body: init.body === undefined || init.body === null ? null : String(init.body),
        timeoutMs: init.timeoutMs || 30000
      });
    },
    oauth: {
      authorize: function (opts) {
        opts = opts || {};
        return call("oauth.authorize", {
          url: String(opts.url),
          redirectPrefix: String(opts.redirectPrefix),
          timeoutMs: opts.timeoutMs || 300000
        });
      }
    },
    openUrl: function (url) { return call("openUrl", { url: String(url) }); },
    page: { current: function () { return call("page.current"); } },
    ui: { close: function () { return call("ui.close"); } },
    agent: {
      state: function () { return call("agent.state"); },
      setModel: function (id) { return call("agent.setModel", { id: String(id) }); },
      send: function (text) { return call("agent.send", { text: String(text) }); },
      stop: function () { return call("agent.stop"); },
      clear: function () { return call("agent.clear"); }
    },
    voice: {
      state:  function () { return call("voice.state"); },
      start:  function () { return call("voice.start"); },
      stop:   function () { return call("voice.stop"); },
      cancel: function () { return call("voice.cancel"); }
    },
    on: function (name, fn) {
      if (typeof fn !== "function") throw new TypeError("copper.on needs a function");
      (listeners[name] = listeners[name] || []).push(fn);
      return function off() {
        var set = listeners[name];
        if (!set) return;
        var i = set.indexOf(fn);
        if (i >= 0) set.splice(i, 1);
      };
    }
  };
  Object.freeze(copper.storage); Object.freeze(copper.oauth); Object.freeze(copper.page);
  Object.freeze(copper.voice);
  Object.freeze(copper.ui); Object.freeze(copper.agent); Object.freeze(copper);
  Object.defineProperty(window, "copper", { value: copper, writable: false, configurable: false });
  try { window.dispatchEvent(new Event("copperready")); } catch (_) {}
})();
