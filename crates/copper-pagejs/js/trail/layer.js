// from Sources/Search/Fork/MCP/Trail.swift:146 Trail.script
// Linux: installer, run before every trail/* call (idempotent: re-ensures the layer when present).
function () {
  var ID = '__jev-trail';
  var NS = 'http://www.w3.org/2000/svg';
  var ACCENT = '#c8743c';
  var HALO = 'rgba(255,255,255,0.62)';   // so a line reads on black and on white
  var KEEP = 10;                         // trail segments kept
  var FADE = [1, 0.75, 0.5, 0.35];       // and how they step down with age

  // Already here: make sure the element still is, and stop.
  if (window.__jevTrail && typeof window.__jevTrail.ensure === 'function') {
    window.__jevTrail.ensure();
    return true;
  }

  function blank() {
    return { svg: null, scan: null, page: null, view: null, cursor: null,
             clicks: [], box: null, tag: null, tagTimer: 0, count: 0, at: null, frame: false };
  }
  var state = blank();
  var timers = [];

  function el(tag, props) {
    var node = document.createElementNS(NS, tag);
    for (var key in props) if (Object.prototype.hasOwnProperty.call(props, key)) node.setAttribute(key, props[key]);
    return node;
  }
  function drop(node) { if (node && node.parentNode) node.parentNode.removeChild(node); }
  function later(fn, ms) { var id = setTimeout(fn, ms); timers.push(id); return id; }
  function num(value) { var n = +value; return isFinite(n) ? n : 0; }

  // A small rAF tween — attributes, not CSS transforms, so nothing
  // depends on how a browser resolves transform-origin inside an SVG.
  // A tab in the background has no frames at all, so a timer stands
  // behind every tween: it lands on the last frame and finishes, and
  // nothing is left half-drawn or waiting on a promise that never came.
  function ease(t) { return 1 - Math.pow(1 - t, 3); }
  function tween(ms, step, done) {
    ms = Math.max(1, ms);
    var start = null, over = false, guard = 0;
    function settle() {
      if (over) return;
      over = true;
      clearTimeout(guard);
      step(1);
      if (done) done();
    }
    function frame(now) {
      if (over) return;
      if (start === null) start = now;
      var t = Math.min(1, (now - start) / ms);
      if (t >= 1) { settle(); return; }
      step(t);
      requestAnimationFrame(frame);
    }
    guard = later(settle, ms + 160);
    requestAnimationFrame(frame);
  }
  function fade(node, ms) {
    later(function () {
      tween(260, function (t) { node.setAttribute('opacity', 1 - t); }, function () { drop(node); });
    }, ms);
  }

  // The pointer: a macOS arrow whose tip is the origin, so translating it
  // to a point puts the point under the tip.
  var ARROW = 'M0,0 L0,17.4 L4.4,13.3 L7.0,18.8 L9.7,17.5 L7.1,12.2 L12.6,12.0 Z';

  function cursorGroup() {
    var group = el('g', { opacity: 0, filter: 'url(#__jev-trail-shade)' });
    // The white edge is painted under the fill, so the arrow keeps its
    // colour and only its outside goes white.
    var arrow = el('path', { d: ARROW, fill: ACCENT, stroke: '#ffffff', 'stroke-width': 2.5,
                             'stroke-linejoin': 'round' });
    arrow.style.paintOrder = 'stroke fill';
    group.appendChild(arrow);
    return group;
  }
  function place(x, y, scale) {
    if (!state.cursor) return;
    state.cursor.setAttribute('transform', 'translate(' + x + ',' + y + ') scale(' + scale + ')');
  }

  // The page may have removed the layer; rebuild it and forget the trail.
  function ensure() {
    var found = document.getElementById(ID);
    if (found && found.isConnected && found === state.svg) return state.svg;
    drop(found);
    var count = state.count;
    state = blank();
    state.count = count;
    var root = document.documentElement || document.body;
    if (!root) return null;
    var svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('id', ID);
    svg.setAttribute('aria-hidden', 'true');
    svg.style.cssText = 'position:fixed;inset:0;top:0;left:0;width:100%;height:100%;' +
      'pointer-events:none;z-index:2147483647;margin:0;padding:0;border:0;background:none;';
    var defs = el('defs', {});
    var shade = el('filter', { id: '__jev-trail-shade', x: '-60%', y: '-60%', width: '260%', height: '260%' });
    shade.appendChild(el('feDropShadow', { dx: 0, dy: 1, stdDeviation: 1.3,
                                           'flood-color': '#000000', 'flood-opacity': 0.35 }));
    defs.appendChild(shade);
    svg.appendChild(defs);
    var scan = el('g', {});   // viewport coordinates, the read pulse
    var page = el('g', {});   // page coordinates, re-projected on scroll
    var view = el('g', {});   // viewport coordinates, for edge glyphs
    var cursor = cursorGroup();
    svg.appendChild(scan);
    svg.appendChild(page);
    svg.appendChild(view);
    svg.appendChild(cursor);
    root.appendChild(svg);
    state.svg = svg; state.scan = scan; state.page = page; state.view = view; state.cursor = cursor;
    project();
    return svg;
  }

  function project() {
    if (!state.page) return;
    state.page.setAttribute('transform', 'translate(' + (-(window.scrollX || 0)) + ',' + (-(window.scrollY || 0)) + ')');
  }
  function onScroll() {
    if (state.frame) return;
    state.frame = true;
    requestAnimationFrame(function () { state.frame = false; project(); });
  }

  // MARK: the pointer

  // From wherever the pointer was (the first time: the middle of the
  // view, fading in) to the point, ease-out, and the promise settles when
  // it lands.
  function glide(a) {
    if (!ensure()) return Promise.resolve(true);
    var x = num(a && a.x), y = num(a && a.y);
    var first = !state.at;
    var from = state.at || { x: (window.innerWidth || 800) / 2, y: (window.innerHeight || 600) / 2 };
    var far = Math.sqrt(Math.pow(x - from.x, 2) + Math.pow(y - from.y, 2));
    var ms = Math.min(320, 120 + far * 0.35);
    var cursor = state.cursor;
    place(from.x, from.y, 1);
    cursor.setAttribute('opacity', first ? 0 : 1);
    return new Promise(function (resolve) {
      tween(ms, function (t) {
        var e = ease(t);
        place(from.x + (x - from.x) * e, from.y + (y - from.y) * e, 1);
        if (first) cursor.setAttribute('opacity', e);
      }, function () {
        state.at = { x: x, y: y };
        place(x, y, 1);
        cursor.setAttribute('opacity', 1);
        resolve(true);
      });
    });
  }

  // Nothing to do but watch: two slow breaths, and back to idle.
  function wait() {
    if (!ensure()) return true;
    if (!state.at) {
      state.at = { x: (window.innerWidth || 800) / 2, y: (window.innerHeight || 600) / 2 };
      place(state.at.x, state.at.y, 1);
    }
    var cursor = state.cursor;
    tween(1300, function (t) {
      cursor.setAttribute('opacity', 0.75 - 0.25 * Math.cos(t * 4 * Math.PI));
    }, function () { cursor.setAttribute('opacity', 0.7); });
    return true;
  }

  // MARK: the target and its tag

  function tagFor(operation, label) {
    var op = String(operation || '').toUpperCase();
    var word = op === 'TYPE_TEXT' ? 'TYPE' : op;
    var name = String(label || '').replace(/\s+/g, ' ').trim();
    if (name.length > 28) name = name.slice(0, 27) + '…';
    if (word && name) return word + ' · ' + name;
    return word || name;
  }

  // The element Jev is about to act on: an outline that settles onto the
  // box from 8 px out with the move named just above it, one at a time.
  function target(a) {
    if (!ensure()) return true;
    var r = a && a.rect;
    if (!r) return true;
    var w = Math.max(2, num(r.w)), h = Math.max(2, num(r.h));
    var x = num(r.x) + (window.scrollX || 0), y = num(r.y) + (window.scrollY || 0);
    drop(state.box);
    drop(state.tag);
    if (state.tagTimer) { clearTimeout(state.tagTimer); state.tagTimer = 0; }
    var box = el('rect', {
      x: x, y: y, width: w, height: h, rx: 6, ry: 6,
      fill: ACCENT, 'fill-opacity': 0.08,
      stroke: ACCENT, 'stroke-opacity': 0.6, 'stroke-width': 1.5
    });
    var halo = el('rect', {
      x: x, y: y, width: w, height: h, rx: 6, ry: 6,
      fill: 'none', stroke: HALO, 'stroke-width': 3, 'stroke-opacity': 0.35
    });
    var group = el('g', {});
    group.appendChild(halo);
    group.appendChild(box);
    state.page.appendChild(group);
    state.box = group;
    tween(180, function (t) {
      var out = 8 * (1 - ease(t));
      [halo, box].forEach(function (node) {
        node.setAttribute('x', x - out);
        node.setAttribute('y', y - out);
        node.setAttribute('width', w + out * 2);
        node.setAttribute('height', h + out * 2);
      });
      group.setAttribute('opacity', 0.35 + 0.65 * t);
    });

    var text = tagFor(a.operation, a.label);
    if (!text) return true;
    var tag = el('g', { opacity: 0 });
    var plate = el('rect', { rx: 4, ry: 4, fill: ACCENT, 'fill-opacity': 0.95,
                             stroke: HALO, 'stroke-width': 1, 'stroke-opacity': 0.3 });
    var label = el('text', {
      x: 0, y: 0, fill: '#ffffff', 'font-size': '10.5px', 'font-weight': '600',
      'font-family': 'system-ui, -apple-system, sans-serif', 'dominant-baseline': 'hanging'
    });
    label.textContent = text;
    tag.appendChild(plate);
    tag.appendChild(label);
    state.page.appendChild(tag);
    state.tag = tag;
    var size = { width: text.length * 6, height: 12 };
    try { var measured = label.getBBox(); if (measured.width) size = measured; } catch (e) {}
    var tw = size.width + 12, th = size.height + 6;
    var above = num(r.y) > th + 6;                 // room over the box, in the view
    var left = Math.max(2 + (window.scrollX || 0), x);
    var top = above ? y - th - 4 : y + h + 4;
    plate.setAttribute('x', left);
    plate.setAttribute('y', top);
    plate.setAttribute('width', tw);
    plate.setAttribute('height', th);
    label.setAttribute('x', left + 6);
    label.setAttribute('y', top + 3);
    tween(140, function (t) {
      tag.setAttribute('opacity', t);
      tag.setAttribute('transform', 'translate(0,' + (above ? 3 : -3) * (1 - t) + ')');
    });
    return true;
  }

  // The tag has said its piece: away with it. The outline stays until the
  // next target takes its place.
  function retire(ms) {
    if (state.tagTimer) clearTimeout(state.tagTimer);
    var tag = state.tag;
    if (!tag) return;
    state.tagTimer = later(function () {
      state.tagTimer = 0;
      if (state.tag === tag) state.tag = null;
      tween(260, function (t) { tag.setAttribute('opacity', 1 - t); }, function () { drop(tag); });
    }, ms);
  }

  // MARK: the click

  // The press: the dot, its number, the segment back to the last one
  // drawing itself, and two rings that open and go.
  function click(a) {
    if (!ensure()) return true;
    var vx = num(a && a.x), vy = num(a && a.y);
    var x = vx + (window.scrollX || 0), y = vy + (window.scrollY || 0);

    // the pointer dips, then idles
    if (!state.at) { state.at = { x: vx, y: vy }; state.cursor.setAttribute('opacity', 1); }
    var px = state.at.x, py = state.at.y;
    tween(120, function (t) { place(px, py, 0.85 + 0.15 * ease(t)); }, function () {
      place(px, py, 1);
      state.cursor.setAttribute('opacity', 0.7);
    });

    state.count += 1;
    var group = el('g', {});
    var last = state.clicks.length ? state.clicks[state.clicks.length - 1] : null;
    if (last) {
      var far = Math.max(1, Math.sqrt(Math.pow(x - last.x, 2) + Math.pow(y - last.y, 2)));
      var line = { x1: last.x, y1: last.y, x2: x, y2: y, 'stroke-linecap': 'round',
                   'stroke-dasharray': far, 'stroke-dashoffset': far };
      var halo = el('line', Object.assign({}, line, { stroke: HALO, 'stroke-width': 4, 'stroke-opacity': 0.6 }));
      var ink = el('line', Object.assign({}, line, { stroke: ACCENT, 'stroke-width': 2 }));
      group.appendChild(halo);
      group.appendChild(ink);
      tween(220, function (t) {
        var left = far * (1 - ease(t));
        halo.setAttribute('stroke-dashoffset', left);
        ink.setAttribute('stroke-dashoffset', left);
      });
    }
    group.appendChild(el('circle', { cx: x, cy: y, r: 3, fill: ACCENT, stroke: HALO, 'stroke-width': 1.5 }));

    var badge = el('g', {});
    badge.appendChild(el('circle', { cx: x + 10, cy: y - 10, r: 7, fill: ACCENT,
                                     stroke: HALO, 'stroke-width': 1, 'stroke-opacity': 0.5 }));
    var number = el('text', {
      x: x + 10, y: y - 10, fill: '#ffffff', 'font-size': '9.5px', 'font-weight': '700',
      'font-family': 'system-ui, -apple-system, sans-serif',
      'text-anchor': 'middle', 'dominant-baseline': 'central'
    });
    number.textContent = String(state.count);
    badge.appendChild(number);
    group.appendChild(badge);

    state.page.appendChild(group);
    state.clicks.push({ x: x, y: y, node: group });

    [[0, 14], [90, 22]].forEach(function (ring) {
      later(function () {
        var node = el('circle', { cx: x, cy: y, r: 3, fill: 'none', stroke: ACCENT, 'stroke-width': 1.25 });
        state.page.appendChild(node);
        tween(550, function (t) {
          node.setAttribute('r', 3 + (ring[1] - 3) * ease(t));
          node.setAttribute('opacity', 0.85 * (1 - t));
        }, function () { drop(node); });
      }, ring[0]);
    });

    while (state.clicks.length > KEEP) drop(state.clicks.shift().node);
    for (var i = 0; i < state.clicks.length; i++) {
      var age = state.clicks.length - 1 - i;
      state.clicks[i].node.setAttribute('opacity', FADE[Math.min(age, FADE.length - 1)]);
    }
    retire(900);
    return true;
  }

  // MARK: what was written

  // The value, rising by the field and typed out, gone in 4 s.
  function typed(a) {
    if (!ensure()) return true;
    var text = String((a && a.text) || '');
    if (!text) return true;
    if (text.length > 40) text = text.slice(0, 39) + '…';
    var x = num(a && a.x) + (window.scrollX || 0), y = num(a && a.y) + (window.scrollY || 0);
    var group = el('g', { opacity: 0 });
    var plate = el('rect', { rx: 5, ry: 5, fill: ACCENT, 'fill-opacity': 0.94,
                             stroke: HALO, 'stroke-width': 1, 'stroke-opacity': 0.3 });
    var keys = el('g', { fill: '#ffffff', 'fill-opacity': 0.9 });
    var label = el('text', {
      x: 0, y: 0, fill: '#ffffff',
      'font-size': '11px', 'font-family': 'system-ui, -apple-system, sans-serif',
      'dominant-baseline': 'hanging'
    });
    label.textContent = text;
    group.appendChild(plate);
    group.appendChild(keys);
    group.appendChild(label);
    state.page.appendChild(group);

    var size = { width: text.length * 6, height: 13 };
    try { var measured = label.getBBox(); if (measured.width) size = measured; } catch (e) {}
    var pad = 7, glyph = 13, left = x + 10, top = y + 12;
    var h = Math.max(size.height + 8, 18);
    plate.setAttribute('x', left);
    plate.setAttribute('y', top);
    // the plate starts as just the keyboard and grows with the letters
    var base = pad * 2 + glyph + 5, full = size.width + base;
    plate.setAttribute('width', base);
    plate.setAttribute('height', h);
    // a keyboard, three rounded bars, not a character
    var kx = left + pad, ky = top + h / 2 - 4.5;
    [[0, 0, 11, 2.4], [0, 3.4, 11, 2.4], [2.6, 6.8, 5.8, 2.4]].forEach(function (bar) {
      keys.appendChild(el('rect', { x: kx + bar[0], y: ky + bar[1], width: bar[2], height: bar[3], rx: 1, ry: 1 }));
    });
    label.setAttribute('x', left + pad + glyph + 5);
    label.setAttribute('y', top + (h - size.height) / 2);

    label.textContent = '';
    tween(120, function (t) {
      group.setAttribute('opacity', t);
      group.setAttribute('transform', 'translate(0,' + 4 * (1 - t) + ')');
    }, function () {
      group.setAttribute('transform', 'translate(0,0)');
      tween(Math.min(300, text.length * 14), function (t) {
        label.textContent = text.slice(0, Math.max(1, Math.ceil(text.length * t)));
        plate.setAttribute('width', base + (full - base) * t);
      }, function () { plate.setAttribute('width', full); });
    });
    fade(group, 4000);
    retire(900);
    return true;
  }

  // MARK: what Jev sees

  // Every control the read handed over, lit for a moment, top to bottom.
  function seen(a) {
    if (!ensure()) return true;
    var list = (a && a.rects) || [];
    var keys = {}, out = [];
    for (var i = 0; i < list.length; i++) {
      var r = list[i]; if (!r) continue;
      var w = num(r.w), h = num(r.h), x = num(r.x), y = num(r.y);
      if (w < 2 || h < 2) continue;
      var key = x + ':' + y + ':' + w + ':' + h;
      if (keys[key]) continue;
      keys[key] = 1;
      out.push({ x: x, y: y, w: w, h: h });
    }
    if (!out.length) return true;
    out.sort(function (p, q) { return q.w * q.h - p.w * p.h; });
    out = out.slice(0, 120);
    out.sort(function (p, q) { return p.y - q.y; });
    var group = el('g', {});
    state.scan.appendChild(group);
    var span = out.length > 1 ? 120 / (out.length - 1) : 0;
    out.forEach(function (r, index) {
      var node = el('rect', {
        x: r.x, y: r.y, width: r.w, height: r.h, rx: 3, ry: 3, opacity: 0,
        fill: ACCENT, 'fill-opacity': 0.04, stroke: ACCENT, 'stroke-opacity': 0.35, 'stroke-width': 1
      });
      group.appendChild(node);
      later(function () { tween(110, function (t) { node.setAttribute('opacity', t); }); }, span * index);
    });
    later(function () {
      tween(700, function (t) { group.setAttribute('opacity', 1 - t); }, function () { drop(group); });
    }, 220);
    return true;
  }

  // MARK: scroll

  // Which way the page moved: a chevron and a short line of motion, both
  // drifting that way as they go.
  function scroll(a) {
    if (!ensure()) return true;
    var down = num(a && a.delta) >= 0;
    var w = window.innerWidth || 800, h = window.innerHeight || 600;
    var cx = w / 2, cy = down ? h - 46 : 46, arm = 13, rise = down ? -9 : 9;
    var group = el('g', {});
    [[HALO, 4, 0.5], [ACCENT, 1.5, 0.95]].forEach(function (pen) {
      var stroke = { stroke: pen[0], 'stroke-width': pen[1], 'stroke-opacity': pen[2], 'stroke-linecap': 'round' };
      group.appendChild(el('line', Object.assign({ x1: cx - arm, y1: cy + rise, x2: cx, y2: cy }, stroke)));
      group.appendChild(el('line', Object.assign({ x1: cx + arm, y1: cy + rise, x2: cx, y2: cy }, stroke)));
      group.appendChild(el('line', Object.assign({ x1: cx, y1: cy + rise * 2.4, x2: cx, y2: cy + rise * 5.2 }, stroke)));
    });
    state.view.appendChild(group);
    tween(650, function (t) {
      group.setAttribute('opacity', 1 - t);
      group.setAttribute('transform', 'translate(0,' + (down ? 18 : -18) * ease(t) + ')');
    }, function () { drop(group); });
    return true;
  }

  function clear() {
    window.removeEventListener('scroll', onScroll);
    window.removeEventListener('resize', onScroll);
    for (var i = 0; i < timers.length; i++) clearTimeout(timers[i]);
    timers = [];
    drop(document.getElementById(ID));
    state = blank();
    try { delete window.__jevTrail; } catch (e) { window.__jevTrail = null; }
    return true;
  }

  window.addEventListener('scroll', onScroll, { passive: true });
  window.addEventListener('resize', onScroll, { passive: true });
  window.__jevTrail = { ensure: ensure, target: target, glide: glide, click: click,
                        typed: typed, seen: seen, scroll: scroll, wait: wait, clear: clear };
  ensure();
  return true;
}
