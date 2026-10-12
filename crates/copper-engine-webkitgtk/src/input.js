// Copper input: the script half of copper_engine::PageView::input.
//
// Two jobs. (1) Deliver an InputEvent by dispatching DOM events from script
// (isTrusted == false): the fallback for actions WebKit has no trusted path
// for (right/middle/modified clicks, drag, wheel, most keys) and for pages
// whose WebKit event controllers failed the self-check. (2) Help the Rust
// side deliver trusted input (T-002 / D-006): `arm`/`settle` around pointer
// actions emitted on WebKit's own GTK event controllers, `keys`/`fallback`
// around WebKit editing commands.
//
// Runs as an async function body in the isolated world "copper-input" with
// one argument, `ev`: the externally tagged JSON of an InputEvent, e.g.
// {"click": {"x": 10, "y": 20, ...}} or {"type": "hello"}, or of one of the
// helper ops above. Returns {ok: true, target, ...} or {ok: false, error}.
const kind = typeof ev === 'string' ? ev : Object.keys(ev)[0];
const a = typeof ev === 'string' ? {} : ev[kind];
const S = (globalThis.__copperInput ||= { x: NaN, y: NaN, over: null });
const BUTTON = { left: 0, middle: 1, right: 2 };
const BUTTONS = { left: 1, middle: 4, right: 2 };

const mods = (m) => ({
  shiftKey: !!(m && m.shift),
  ctrlKey: !!(m && m.ctrl),
  altKey: !!(m && m.alt),
  metaKey: !!(m && m.meta),
});
const winOf = (el) => (el && el.ownerDocument && el.ownerDocument.defaultView) || window;
const describe = (el) =>
  el ? el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') : null;

// Element under a viewport point, descending into open shadow roots and
// same-origin frames. Returns the element plus the point in its frame.
function hit(x, y) {
  let ox = 0;
  let oy = 0;
  let el = document.elementFromPoint(x, y);
  for (let depth = 0; el && depth < 32; depth++) {
    if (el.shadowRoot) {
      const inner = el.shadowRoot.elementFromPoint(x - ox, y - oy);
      if (inner && inner !== el) {
        el = inner;
        continue;
      }
    }
    if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') {
      let doc = null;
      try {
        doc = el.contentDocument;
      } catch (_) {}
      if (!doc) break;
      const r = el.getBoundingClientRect();
      ox += r.left + el.clientLeft;
      oy += r.top + el.clientTop;
      const inner = doc.elementFromPoint(x - ox, y - oy);
      if (!inner) break;
      el = inner;
      continue;
    }
    break;
  }
  return el ? { el, x: x - ox, y: y - oy, win: winOf(el) } : null;
}

function mouse(t, type, extra) {
  const W = t.win;
  const init = Object.assign(
    {
      bubbles: true,
      cancelable: true,
      composed: true,
      view: W,
      clientX: t.x,
      clientY: t.y,
      screenX: t.x,
      screenY: t.y,
    },
    extra,
  );
  let e;
  if (type.startsWith('pointer') && W.PointerEvent) {
    e = new W.PointerEvent(
      type,
      Object.assign(
        { pointerId: 1, pointerType: 'mouse', isPrimary: true, width: 1, height: 1 },
        { pressure: init.buttons ? 0.5 : 0 },
        init,
      ),
    );
  } else if (type.startsWith('pointer')) {
    return true;
  } else if (type === 'wheel') {
    e = new W.WheelEvent(type, init);
  } else {
    e = new W.MouseEvent(type, init);
  }
  return t.el.dispatchEvent(e);
}

// Ancestors (deepest first), crossing shadow boundaries.
function chain(n) {
  const out = [];
  while (n) {
    if (n.nodeType === 1) out.push(n);
    n = n.parentNode || n.host || null;
  }
  return out;
}

function hover(t, m, buttons) {
  const base = Object.assign(mods(m), { buttons: buttons || 0 });
  const prev = S.over && S.over.isConnected ? S.over : null;
  if (prev !== t.el) {
    const oldC = prev ? chain(prev) : [];
    const newC = chain(t.el);
    const noBubble = { bubbles: false, cancelable: false };
    if (prev) {
      const pt = { el: prev, x: t.x, y: t.y, win: winOf(prev) };
      mouse(pt, 'pointerout', Object.assign({ relatedTarget: t.el }, base));
      mouse(pt, 'mouseout', Object.assign({ relatedTarget: t.el }, base));
      for (const n of oldC) {
        if (newC.includes(n)) continue;
        const nt = { el: n, x: t.x, y: t.y, win: winOf(n) };
        mouse(nt, 'pointerleave', Object.assign({ relatedTarget: t.el }, base, noBubble));
        mouse(nt, 'mouseleave', Object.assign({ relatedTarget: t.el }, base, noBubble));
      }
    }
    mouse(t, 'pointerover', Object.assign({ relatedTarget: prev }, base));
    mouse(t, 'mouseover', Object.assign({ relatedTarget: prev }, base));
    for (const n of newC.slice().reverse()) {
      if (oldC.includes(n)) continue;
      const nt = { el: n, x: t.x, y: t.y, win: winOf(n) };
      mouse(nt, 'pointerenter', Object.assign({ relatedTarget: prev }, base, noBubble));
      mouse(nt, 'mouseenter', Object.assign({ relatedTarget: prev }, base, noBubble));
    }
    S.over = t.el;
  }
  mouse(t, 'pointermove', base);
  mouse(t, 'mousemove', base);
}

const FOCUSABLE =
  'input:not([disabled]),textarea:not([disabled]),select:not([disabled]),' +
  'button:not([disabled]),a[href],area[href],iframe,summary,[tabindex],' +
  '[contenteditable=""],[contenteditable="true"]';

// What a real mousedown would focus: the nearest focusable ancestor.
function focusFor(el) {
  for (let n = el; n; n = n.parentElement || (n.getRootNode && n.getRootNode().host) || null) {
    if (n.nodeType === 1 && n.matches && n.matches(FOCUSABLE)) return n;
  }
  return null;
}

function click(a) {
  const t = hit(a.x, a.y);
  if (!t) return { ok: false, error: `no element at ${a.x},${a.y}` };
  S.x = a.x;
  S.y = a.y;
  const b = BUTTON[a.button] ?? 0;
  const bs = BUTTONS[a.button] ?? 1;
  const m = mods(a.modifiers);
  const count = Math.max(1, a.count | 0);
  hover(t, a.modifiers, 0);
  for (let i = 1; i <= count; i++) {
    const down = Object.assign({ button: b, buttons: bs, detail: i }, m);
    const up = Object.assign({ button: b, buttons: 0, detail: i }, m);
    // A cancelled pointerdown suppresses the compatibility mouse events.
    const pd = mouse(t, 'pointerdown', down);
    const md = pd ? mouse(t, 'mousedown', down) : false;
    if (md && b === 0) {
      const f = focusFor(t.el);
      if (f) f.focus();
      else {
        const doc = t.el.ownerDocument;
        if (doc.activeElement && doc.activeElement !== doc.body) doc.activeElement.blur();
      }
    }
    mouse(t, 'pointerup', up);
    if (pd) mouse(t, 'mouseup', up);
    if (b === 0) mouse(t, 'click', up);
    else mouse(t, 'auxclick', up);
    if (b === 2 && i === 1) mouse(t, 'contextmenu', up);
  }
  if (b === 0 && count >= 2) mouse(t, 'dblclick', Object.assign({ button: 0, buttons: 0, detail: 2 }, m));
  return { ok: true, target: describe(t.el) };
}

function move(a) {
  const t = hit(a.x, a.y);
  if (!t) return { ok: false, error: `no element at ${a.x},${a.y}` };
  S.x = a.x;
  S.y = a.y;
  hover(t, a.modifiers, 0);
  return { ok: true, target: describe(t.el) };
}

function dragEvent(t, type, dt) {
  const init = {
    bubbles: true,
    cancelable: type !== 'dragleave' && type !== 'dragend',
    composed: true,
    view: t.win,
    clientX: t.x,
    clientY: t.y,
    dataTransfer: dt,
  };
  let e;
  try {
    e = new t.win.DragEvent(type, init);
  } catch (_) {
    e = new t.win.MouseEvent(type, init);
  }
  return t.el.dispatchEvent(e);
}

async function drag(a) {
  const t0 = hit(a.from_x, a.from_y);
  if (!t0) return { ok: false, error: `no element at ${a.from_x},${a.from_y}` };
  const m = mods(a.modifiers);
  S.x = a.from_x;
  S.y = a.from_y;
  hover(t0, a.modifiers, 0);
  const pd = mouse(t0, 'pointerdown', Object.assign({ button: 0, buttons: 1, detail: 1 }, m));
  if (pd) mouse(t0, 'mousedown', Object.assign({ button: 0, buttons: 1, detail: 1 }, m));
  // HTML drag and drop for draggable="true" sources.
  const src = t0.el.closest ? t0.el.closest('[draggable="true"]') : null;
  let dt = null;
  let dnd = false;
  if (src) {
    try {
      dt = new t0.win.DataTransfer();
    } catch (_) {}
    dnd = dragEvent({ el: src, x: t0.x, y: t0.y, win: t0.win }, 'dragstart', dt);
  }
  const steps = 10;
  let t = t0;
  let over = null;
  let accepted = false;
  for (let i = 1; i <= steps; i++) {
    const x = a.from_x + ((a.to_x - a.from_x) * i) / steps;
    const y = a.from_y + ((a.to_y - a.from_y) * i) / steps;
    t = hit(x, y) || t;
    S.x = x;
    S.y = y;
    if (dnd) {
      if (t.el !== over) {
        if (over) dragEvent({ el: over, x: t.x, y: t.y, win: winOf(over) }, 'dragleave', dt);
        dragEvent(t, 'dragenter', dt);
        over = t.el;
      }
      dragEvent({ el: src, x: t.x, y: t.y, win: t.win }, 'drag', dt);
      accepted = !dragEvent(t, 'dragover', dt);
    } else {
      hover(t, a.modifiers, 1);
    }
    await new Promise((r) => setTimeout(r, 16));
  }
  if (dnd) {
    if (accepted) dragEvent(t, 'drop', dt);
    dragEvent({ el: src, x: t.x, y: t.y, win: t.win }, 'dragend', dt);
  }
  const up = Object.assign({ button: 0, buttons: 0, detail: 1 }, m);
  mouse(t, 'pointerup', up);
  if (pd) mouse(t, 'mouseup', up);
  return { ok: true, target: describe(t.el) };
}

// The focused element, through open shadow roots and same-origin frames.
function deepActive() {
  let el = document.activeElement;
  for (let i = 0; el && i < 32; i++) {
    if (el.shadowRoot && el.shadowRoot.activeElement) {
      el = el.shadowRoot.activeElement;
      continue;
    }
    if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') {
      let doc = null;
      try {
        doc = el.contentDocument;
      } catch (_) {}
      if (doc && doc.activeElement) {
        el = doc.activeElement;
        continue;
      }
    }
    break;
  }
  return el || document.body || document.documentElement;
}

const TEXT_TYPES = new Set(['text', 'search', 'email', 'url', 'tel', 'password', 'number']);
const isTextField = (el) =>
  el.tagName === 'TEXTAREA' || (el.tagName === 'INPUT' && TEXT_TYPES.has(el.type));

// Set a field's value through the prototype setter, bypassing any
// framework-installed instance setter (React's value tracker), so the
// framework sees the change when `input` fires.
function setValue(el, v) {
  const W = winOf(el);
  const proto = el.tagName === 'TEXTAREA' ? W.HTMLTextAreaElement.prototype : W.HTMLInputElement.prototype;
  const d = Object.getOwnPropertyDescriptor(proto, 'value');
  if (d && d.set) d.set.call(el, v);
  else el.value = v;
}

function inputEvent(el, type, inputType, data, cancelable) {
  const W = winOf(el);
  return el.dispatchEvent(
    new W.InputEvent(type, { bubbles: true, cancelable: !!cancelable, composed: true, inputType, data }),
  );
}

// Apply one editing operation to a text field or contenteditable element.
function edit(el, inputType, data) {
  if (isTextField(el)) {
    if (el.readOnly || el.disabled) return false;
    const insert = inputType === 'insertText' ? data : inputType === 'insertLineBreak' ? '\n' : null;
    if (!inputEvent(el, 'beforeinput', inputType, insert, true)) return false;
    const v = el.value;
    let s = null;
    let e = null;
    try {
      s = el.selectionStart;
      e = el.selectionEnd;
    } catch (_) {}
    if (s == null || e == null) s = e = v.length;
    let next;
    let caret;
    if (insert != null) {
      next = v.slice(0, s) + insert + v.slice(e);
      caret = s + insert.length;
    } else if (inputType === 'deleteContentBackward') {
      const from = s === e ? Math.max(0, s - 1) : s;
      next = v.slice(0, from) + v.slice(e);
      caret = from;
    } else {
      const to = s === e ? Math.min(v.length, e + 1) : e;
      next = v.slice(0, s) + v.slice(to);
      caret = s;
    }
    setValue(el, next);
    try {
      el.setSelectionRange(caret, caret);
    } catch (_) {}
    inputEvent(el, 'input', inputType, insert, false);
    return true;
  }
  if (el.isContentEditable) {
    const cmd = {
      insertText: 'insertText',
      insertLineBreak: 'insertParagraph',
      deleteContentBackward: 'delete',
      deleteContentForward: 'forwardDelete',
    }[inputType];
    return el.ownerDocument.execCommand(cmd, false, data ?? undefined);
  }
  return false;
}

function tabMove(el, back) {
  const doc = el.ownerDocument;
  const all = Array.from(doc.querySelectorAll(FOCUSABLE)).filter(
    (n) => n.tabIndex >= 0 && n.getClientRects().length > 0,
  );
  if (!all.length) return;
  let i = all.indexOf(el);
  if (i < 0) i = back ? all.length - 1 : 0;
  else i = (i + (back ? -1 : 1) + all.length) % all.length;
  all[i].focus();
}

// What a real key press would do when nobody cancelled keydown.
function defaultAction(el, key, m) {
  switch (key) {
    case 'Enter':
      if (el.tagName === 'TEXTAREA' || el.isContentEditable) return edit(el, 'insertLineBreak');
      if (el.tagName === 'INPUT' && el.form) {
        const form = el.form;
        const submitter = form.querySelector('button:not([type]),button[type=submit],input[type=submit]');
        if (submitter) submitter.click();
        else if (form.requestSubmit) form.requestSubmit();
        else form.submit();
      } else if (el.tagName === 'BUTTON' || (el.tagName === 'A' && el.href)) el.click();
      return true;
    case 'Backspace':
      return edit(el, 'deleteContentBackward');
    case 'Delete':
      return edit(el, 'deleteContentForward');
    case 'Tab':
      tabMove(el, !!(m && m.shift));
      return true;
    default:
      return false;
  }
}

const KEYCODES = {
  Enter: 13, Tab: 9, Backspace: 8, Delete: 46, Escape: 27, ' ': 32,
  ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
  Home: 36, End: 35, PageUp: 33, PageDown: 34, Shift: 16, Control: 17, Alt: 18, Meta: 91,
};
function codeFor(key) {
  if (/^[a-z]$/i.test(key)) return 'Key' + key.toUpperCase();
  if (/^[0-9]$/.test(key)) return 'Digit' + key;
  if (key === ' ') return 'Space';
  return key.length === 1 ? '' : key;
}
function keyCodeFor(key) {
  if (key in KEYCODES) return KEYCODES[key];
  return key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0;
}

function press(key, text, m) {
  const el = deepActive();
  const W = winOf(el);
  const kc = keyCodeFor(key);
  const init = Object.assign(
    { key, code: codeFor(key), bubbles: true, cancelable: true, composed: true, view: W, keyCode: kc, which: kc },
    mods(m),
  );
  const down = el.dispatchEvent(new W.KeyboardEvent('keydown', init));
  if (down) {
    if (text) {
      const cc = text.codePointAt(0);
      const kp = el.dispatchEvent(
        new W.KeyboardEvent('keypress', Object.assign({}, init, { charCode: cc, keyCode: cc, which: cc })),
      );
      if (kp) edit(el, 'insertText', text);
    } else {
      defaultAction(el, key, m);
    }
  }
  el.dispatchEvent(new W.KeyboardEvent('keyup', init));
  return el;
}

function scroll(a) {
  const x = Number.isFinite(S.x) ? S.x : innerWidth / 2;
  const y = Number.isFinite(S.y) ? S.y : innerHeight / 2;
  const t = hit(x, y) || { el: document.scrollingElement || document.documentElement, x, y, win: window };
  if (!mouse(t, 'wheel', { deltaX: a.dx, deltaY: a.dy, deltaMode: 0 })) return { ok: true, target: describe(t.el) };
  for (let n = t.el; n && n.nodeType === 1; n = n.parentElement) {
    const doc = n.ownerDocument;
    if (n === doc.scrollingElement || n === doc.body || n === doc.documentElement) break;
    const cs = winOf(n).getComputedStyle(n);
    const canY = a.dy && /(auto|scroll|overlay)/.test(cs.overflowY) && n.scrollHeight > n.clientHeight;
    const canX = a.dx && /(auto|scroll|overlay)/.test(cs.overflowX) && n.scrollWidth > n.clientWidth;
    if (canX || canY) {
      n.scrollBy(a.dx, a.dy);
      return { ok: true, target: describe(n) };
    }
  }
  t.win.scrollBy(a.dx, a.dy);
  return { ok: true, target: describe(t.el) };
}

// --- Trusted-input helpers ------------------------------------------------

// Before the engine delivers a pointer action at (x, y): check there is an
// element under the point, remember the point (Scroll uses it), and listen
// for the action's last trusted event (`until`: 'click' with detail >=
// count, or 'mousemove'). `watch` is false when the point is in a
// cross-origin frame, whose events never reach a listener out here.
function arm(a) {
  const t = hit(a.x, a.y);
  if (!t) return { ok: false, error: `no element at ${a.x},${a.y}` };
  S.x = a.x;
  S.y = a.y;
  S.over = t.el;
  if (S.disarm) S.disarm();
  S.pending = null;
  if (t.el.tagName === 'IFRAME' || t.el.tagName === 'FRAME') {
    return { ok: true, target: describe(t.el), watch: false };
  }
  const W = t.win;
  const type = a.until;
  const count = Math.max(1, a.count | 0);
  let done;
  S.pending = new Promise((r) => (done = r));
  const disarm = () => {
    W.removeEventListener(type, on, true);
    if (S.disarm === disarm) S.disarm = null;
  };
  const on = (e) => {
    if (!e.isTrusted || (type === 'click' && (e.detail | 0) < count)) return;
    disarm();
    done(true);
  };
  W.addEventListener(type, on, true);
  S.disarm = disarm;
  return { ok: true, target: describe(t.el), watch: true };
}

// Wait up to `ms` for the event `arm` listens for.
async function settle(a) {
  const p = S.pending;
  S.pending = null;
  if (!p) return { ok: true, seen: false };
  let timer;
  const late = new Promise((r) => (timer = setTimeout(() => r(false), a.ms)));
  const seen = await Promise.race([p, late]);
  clearTimeout(timer);
  if (!seen && S.disarm) S.disarm();
  return { ok: true, seen };
}

const shortcut = (m) => !!(m && (m.ctrl || m.alt || m.meta));
function keyInit(key, m, W) {
  const kc = keyCodeFor(key);
  return Object.assign(
    { key, code: codeFor(key), bubbles: true, cancelable: true, composed: true, view: W, keyCode: kc, which: kc },
    mods(m),
  );
}

// keyup for the previous key (after the engine ran its editing command),
// then keydown (+ keypress when it types) for the next one. `proceed` is
// false when the page cancelled it; `editable` says whether an editing
// command has somewhere to go (unknown inside a cross-origin frame: yes).
function keys(a) {
  if (a.up) {
    const el = deepActive();
    const W = winOf(el);
    el.dispatchEvent(new W.KeyboardEvent('keyup', keyInit(a.up.key, a.up.modifiers, W)));
  }
  if (!a.down) return { ok: true };
  const d = a.down;
  const el = deepActive();
  const W = winOf(el);
  const init = keyInit(d.key, d.modifiers, W);
  let proceed = el.dispatchEvent(new W.KeyboardEvent('keydown', init));
  if (proceed && d.text && !shortcut(d.modifiers)) {
    const cc = d.text.codePointAt(0);
    proceed = el.dispatchEvent(
      new W.KeyboardEvent('keypress', Object.assign({}, init, { charCode: cc, keyCode: cc, which: cc })),
    );
  }
  const opaque = el.tagName === 'IFRAME' || el.tagName === 'FRAME';
  return { ok: true, proceed, editable: opaque || isTextField(el) || el.isContentEditable, target: describe(el) };
}

// A key's default action, from script, when the engine has no editing
// command for it (Tab, Enter on a button or link…).
function fallback(a) {
  const el = deepActive();
  defaultAction(el, a.key, a.modifiers);
  return { ok: true, target: describe(el) };
}

switch (kind) {
  case 'arm':
    return arm(a);
  case 'settle':
    return await settle(a);
  case 'keys':
    return keys(a);
  case 'fallback':
    return fallback(a);
  case 'click':
    return click(a);
  case 'move':
    return move(a);
  case 'drag':
    return await drag(a);
  case 'key': {
    const el = press(a.key, a.text, a.modifiers);
    return { ok: true, target: describe(el) };
  }
  case 'scroll':
    return scroll(a);
  case 'type': {
    const text = typeof a === 'string' ? a : ev.type;
    let el = null;
    for (const ch of text) {
      el = ch === '\n' ? press('Enter', null, null) : press(ch, ch, null);
    }
    return { ok: true, target: describe(el || deepActive()) };
  }
  default:
    return { ok: false, error: 'unknown input event ' + kind };
}
