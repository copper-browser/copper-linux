// The hello addon: calls every bridge v1 method once and prints what came
// back, so a person (or Copper's self-test, through window.__helloResults)
// can see the bridge working. Bridge v1 is CONTRACT §5.
(function () {
  "use strict";
  var results = {};
  window.__helloResults = results;
  window.__helloDone = false;
  var seen = [];
  window.__helloEvents = seen;

  var list = document.getElementById("results");
  var events = document.getElementById("events");

  function short(v) {
    var s;
    try { s = JSON.stringify(v); } catch (_) { s = String(v); }
    if (s === undefined) s = "undefined";
    return s.length > 240 ? s.slice(0, 237) + "…" : s;
  }

  function line(into, ok, name, value) {
    var li = document.createElement("li");
    li.className = ok ? "ok" : "bad";
    var mark = document.createElement("span");
    mark.className = "mark";
    mark.textContent = ok ? "✓" : "✗";
    var n = document.createElement("span");
    n.className = "name";
    n.textContent = name;
    var v = document.createElement("span");
    v.className = "value";
    v.textContent = value;
    li.appendChild(mark);
    li.appendChild(n);
    li.appendChild(v);
    into.appendChild(li);
  }

  // `expect`: "ok" (resolves), or an error code the call must fail with.
  function step(name, run, expect) {
    expect = expect || "ok";
    return Promise.resolve()
      .then(run)
      .then(function (value) {
        var ok = expect === "ok";
        results[name] = { ok: true, value: value === undefined ? null : value };
        line(list, ok, name, short(value));
      }, function (err) {
        var code = err && err.code ? err.code : "error";
        var ok = expect === code;
        results[name] = { ok: false, code: code, message: err && err.message ? err.message : String(err) };
        line(list, ok, name, code + (err && err.message ? " · " + err.message : ""));
      });
  }

  function theme(dark) {
    document.documentElement.classList.toggle("dark", !!dark);
  }

  function run() {
    var c = window.copper;
    list.textContent = "";
    window.__helloDone = false;
    if (!c) {
      line(list, false, "window.copper", "missing");
      return Promise.resolve();
    }
    var chain = Promise.resolve();
    function then(name, fn, expect) {
      chain = chain.then(function () { return step(name, fn, expect); });
    }
    then("info()", function () {
      return c.info().then(function (info) { theme(info && info.theme && info.theme.dark); return info; });
    });
    then("identity()", function () { return c.identity(); });
    then("settings()", function () {
      return c.settings().then(function (s) {
        if (s && s.greeting) document.getElementById("greeting").textContent = s.greeting;
        return s;
      });
    });
    then("storage.set(\"count\")", function () {
      return c.storage.get("count").then(function (n) { return c.storage.set("count", (n || 0) + 1); });
    });
    then("storage.get(\"count\")", function () { return c.storage.get("count"); });
    then("storage.set(\"scratch\")", function () { return c.storage.set("scratch", { at: Date.now() }); });
    then("storage.keys()", function () { return c.storage.keys(); });
    then("storage.remove(\"scratch\")", function () { return c.storage.remove("scratch"); });
    then("fetch(example.com)", function () {
      return c.fetch("https://example.com/", { timeoutMs: 8000 }).then(function (r) {
        return { status: r.status, type: r.headers["content-type"] || null, bytes: (r.body || "").length };
      });
    }, "ok");
    then("fetch(denied.example.org)", function () { return c.fetch("https://denied.example.org/"); }, "denied");
    then("page.current()", function () { return c.page.current(); });
    then("agent.state()", function () { return c.agent.state(); });
    then("agent.send()", function () { return c.agent.send("hello"); }, "unavailable");
    return chain.then(function () { window.__helloDone = true; });
  }

  function event(name, payload) {
    seen.push({ name: name, payload: payload === undefined ? null : payload });
    line(events, true, name, short(payload));
    if (name === "theme") theme(payload && payload.dark);
    if (name === "settings" && payload && payload.greeting) {
      document.getElementById("greeting").textContent = payload.greeting;
    }
  }

  function start() {
    var c = window.copper;
    if (!c) {
      line(list, false, "window.copper", "missing — is this page in Copper’s Agent sidebar?");
      return;
    }
    ["visibility", "theme", "settings", "page", "agent"].forEach(function (name) {
      c.on(name, function (payload) { event(name, payload); });
    });
    document.getElementById("close").addEventListener("click", function () { c.ui.close(); });
    document.getElementById("open").addEventListener("click", function () {
      step("openUrl(example.com)", function () { return c.openUrl("https://example.com/?from=hello-addon"); });
    });
    document.getElementById("oauth").addEventListener("click", function () {
      step("oauth.authorize()", function () {
        return c.oauth.authorize({
          url: "https://example.com/authorize?client_id=hello&redirect_uri=http%3A%2F%2F127.0.0.1%3A8765%2Fcallback",
          redirectPrefix: "http://127.0.0.1:8765/callback",
          timeoutMs: 120000
        });
      });
    });
    document.getElementById("again").addEventListener("click", run);
    run();
  }

  if (window.copper) start();
  else window.addEventListener("copperready", start, { once: true });
})();
