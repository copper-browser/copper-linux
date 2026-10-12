// from Sources/Search/Fork/MCP/Page.swift:138 Page.helper
// Linux: installer, run before every page/* call (idempotent: returns at once when window.__copper exists).
function () {
  if (window.__copper) return true;
  var refs = new Map(), byRef = new Map(), next = 1;
  var logs = [];
  ['log', 'warn', 'error', 'info'].forEach(function (level) {
    var original = console[level];
    console[level] = function () {
      try { logs.push({ level: level, text: Array.prototype.map.call(arguments, function (a) { try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); } }).join(' ') }); if (logs.length > 200) logs.shift(); } catch (e) {}
      return original.apply(console, arguments);
    };
  });
  window.addEventListener('error', function (e) { logs.push({ level: 'error', text: String(e.message) }); });

  function ref(el) {
    var r = refs.get(el);
    if (!r) { r = 'e' + (next++); refs.set(el, r); byRef.set(r, el); }
    return r;
  }
  function find(spec) {
    if (!spec) return null;
    if (spec.indexOf('ref:') === 0) {
      var el = byRef.get(spec.slice(4));
      return el && el.isConnected ? el : null;
    }
    if (spec.indexOf('css:') === 0) spec = spec.slice(4);
    try { return document.querySelector(spec); } catch (e) { return null; }
  }
  function visible(el) {
    if (!(el instanceof Element)) return false;
    var s = getComputedStyle(el);
    if (s.display === 'none' || s.visibility === 'hidden' || s.opacity === '0') return false;
    var r = el.getBoundingClientRect();
    return r.width > 0 || r.height > 0 || el.tagName === 'OPTION';
  }
  var interactiveTags = { A: 1, BUTTON: 1, INPUT: 1, SELECT: 1, TEXTAREA: 1, SUMMARY: 1, OPTION: 1, LABEL: 1 };
  function role(el) {
    var explicit = el.getAttribute('role');
    if (explicit) return explicit;
    var t = el.tagName, type = (el.getAttribute('type') || '').toLowerCase();
    switch (t) {
      case 'A': return el.hasAttribute('href') ? 'link' : 'generic';
      case 'BUTTON': return 'button';
      case 'INPUT':
        if (type === 'checkbox') return 'checkbox';
        if (type === 'radio') return 'radio';
        if (type === 'range') return 'slider';
        if (type === 'submit' || type === 'button' || type === 'reset' || type === 'image') return 'button';
        if (type === 'search') return 'searchbox';
        if (type === 'hidden') return '';
        return 'textbox';
      case 'SELECT': return el.multiple ? 'listbox' : 'combobox';
      case 'TEXTAREA': return 'textbox';
      case 'OPTION': return 'option';
      case 'IMG': return el.alt === '' ? 'presentation' : 'img';
      case 'H1': case 'H2': case 'H3': case 'H4': case 'H5': case 'H6': return 'heading';
      case 'NAV': return 'navigation';
      case 'MAIN': return 'main';
      case 'HEADER': return 'banner';
      case 'FOOTER': return 'contentinfo';
      case 'ASIDE': return 'complementary';
      case 'FORM': return 'form';
      case 'UL': case 'OL': return 'list';
      case 'LI': return 'listitem';
      case 'TABLE': return 'table';
      case 'TR': return 'row';
      case 'TD': case 'TH': return 'cell';
      case 'DIALOG': return 'dialog';
      case 'ARTICLE': return 'article';
      case 'SECTION': return 'region';
      case 'SUMMARY': return 'button';
      case 'DETAILS': return 'group';
      case 'LABEL': return 'label';
      case 'P': case 'SPAN': case 'DIV': case 'STRONG': case 'EM': case 'B': case 'I': case 'SMALL': case 'CODE': case 'PRE': case 'BLOCKQUOTE': return 'text';
      default: return 'generic';
    }
  }
  function isInteractive(el) {
    if (el.hasAttribute('contenteditable') && el.getAttribute('contenteditable') !== 'false') return true;
    if (el.tagName in interactiveTags && role(el) !== '') return true;
    var r = el.getAttribute('role');
    if (r && /button|link|checkbox|radio|tab|menuitem|option|switch|slider|textbox|combobox|searchbox|treeitem/.test(r)) return true;
    if (el.hasAttribute('onclick') || (el.tabIndex >= 0 && el.tagName !== 'BODY' && el.tagName !== 'HTML')) return true;
    return getComputedStyle(el).cursor === 'pointer' && el.children.length < 4;
  }
  function name(el) {
    var aria = el.getAttribute('aria-label'); if (aria) return aria.trim();
    var by = el.getAttribute('aria-labelledby');
    if (by) { var parts = by.split(/\s+/).map(function (id) { var n = document.getElementById(id); return n ? n.textContent.trim() : ''; }).filter(Boolean); if (parts.length) return parts.join(' '); }
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') {
      if (el.labels && el.labels.length) return Array.prototype.map.call(el.labels, function (l) { return l.textContent.trim(); }).join(' ');
      if (el.placeholder) return el.placeholder;
      if (el.title) return el.title;
      if (el.name) return el.name;
      if (el.value && el.type === 'submit') return el.value;
      return '';
    }
    if (el.tagName === 'IMG') return el.alt || el.title || '';
    var text = (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
    if (!text && el.title) text = el.title;
    return text.length > 120 ? text.slice(0, 117) + '…' : text;
  }
  function attrs(el) {
    var out = [];
    var r = role(el);
    if (el.tagName === 'A' && el.href) out.push('/url: ' + el.getAttribute('href'));
    if (r === 'heading') out.push('[level=' + (el.getAttribute('aria-level') || el.tagName.slice(1)) + ']');
    if (r === 'checkbox' || r === 'radio' || r === 'switch') out.push('[checked=' + (el.checked || el.getAttribute('aria-checked') === 'true') + ']');
    if ((r === 'textbox' || r === 'searchbox' || r === 'combobox') && el.value) out.push('[value=' + JSON.stringify(String(el.value).slice(0, 80)) + ']');
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') out.push('[disabled]');
    if (el.getAttribute('aria-expanded')) out.push('[expanded=' + el.getAttribute('aria-expanded') + ']');
    if (el.getAttribute('aria-selected') === 'true') out.push('[selected]');
    if (document.activeElement === el) out.push('[active]');
    return out.join(' ');
  }
  function snapshot(selector, interactiveOnly, limit) {
    var root = selector ? find(selector.indexOf(':') > 0 ? selector : 'css:' + selector) : document.body;
    if (!root) return 'nothing matches ' + selector;
    var lines = [], count = 0;
    function walk(el, depth) {
      if (count >= limit) return;
      if (!(el instanceof Element)) return;
      var tag = el.tagName;
      if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE' || tag === 'SVG' || tag === 'PATH') return;
      if (el.getAttribute('aria-hidden') === 'true') return;
      var r = role(el);
      var shown = visible(el);
      var inter = shown && isInteractive(el);
      var nm = shown ? name(el) : '';
      var ownText = '';
      for (var i = 0; i < el.childNodes.length; i++) { var n = el.childNodes[i]; if (n.nodeType === 3) ownText += n.textContent; }
      ownText = ownText.replace(/\s+/g, ' ').trim();
      var landmark = /navigation|main|banner|contentinfo|complementary|form|dialog|region|article|list|table|heading/.test(r);
      var emit = inter || (!interactiveOnly && shown && (landmark || (ownText && r === 'text') || (r === 'img' && nm)));
      if (emit) {
        var line = '  '.repeat(Math.min(depth, 12)) + '- ' + (r || 'generic');
        var label = inter || r === 'heading' || r === 'img' ? nm : landmark ? (el.getAttribute('aria-label') || '') : ownText;
        if (label) line += ' ' + JSON.stringify(label.length > 120 ? label.slice(0, 117) + '…' : label);
        if (inter) line += ' [ref=' + ref(el) + ']';
        var a = attrs(el); if (a) line += ' ' + a;
        if (!inter && r === 'text' && !landmark && lines.length && lines[lines.length - 1].endsWith(line.trim())) { /* skip duplicate */ } else { lines.push(line); count++; }
      }
      var kids = el.shadowRoot ? Array.prototype.slice.call(el.shadowRoot.children) : [];
      kids = kids.concat(Array.prototype.slice.call(el.children));
      for (var k = 0; k < kids.length; k++) walk(kids[k], emit ? depth + 1 : depth);
    }
    walk(root, 0);
    if (count >= limit) lines.push('… (truncated at ' + limit + ' nodes; pass a selector or interactive: true for less)');
    return lines.join('\n');
  }
  function rect(spec) {
    var el = find(spec);
    if (!el) return { error: 'no element for ' + spec };
    if (el.scrollIntoView) el.scrollIntoView({ block: 'center', inline: 'center' });
    var r = el.getBoundingClientRect();
    // A tiny or offscreen box means a wrapper; the first sized child is the thing.
    if ((r.width < 2 || r.height < 2) && el.firstElementChild) r = el.firstElementChild.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  }
  function fire(el, type, init) { el.dispatchEvent(new (type.indexOf('key') === 0 ? KeyboardEvent : type.indexOf('mouse') === 0 || type === 'click' || type === 'dblclick' || type === 'contextmenu' ? MouseEvent : Event)(type, Object.assign({ bubbles: true, cancelable: true, composed: true }, init || {}))); }
  function setValue(spec, value) {
    var el = find(spec);
    if (!el) return 'no element for ' + spec;
    el.focus && el.focus();
    if (el.isContentEditable) {
      el.textContent = value;
      el.dispatchEvent(new InputEvent('input', { bubbles: true, data: value, inputType: 'insertText' }));
      return 'ok';
    }
    if (el.tagName !== 'INPUT' && el.tagName !== 'TEXTAREA') {
      var inner = el.querySelector('input, textarea, [contenteditable]');
      if (inner) return setValue('css:' + cssPath(inner), value);
      return 'not editable: ' + el.tagName;
    }
    var proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    var d = Object.getOwnPropertyDescriptor(proto, 'value');
    if (d && d.set) d.set.call(el, value); else el.value = value;
    fire(el, 'input'); fire(el, 'change');
    return 'ok';
  }
  function cssPath(el) {
    if (el.id) return '#' + CSS.escape(el.id);
    var path = [];
    while (el && el.nodeType === 1 && el !== document.body) {
      var i = 1, s = el.previousElementSibling;
      while (s) { if (s.tagName === el.tagName) i++; s = s.previousElementSibling; }
      path.unshift(el.tagName.toLowerCase() + ':nth-of-type(' + i + ')');
      el = el.parentElement;
    }
    return 'body > ' + path.join(' > ');
  }
  function select(spec, values) {
    var el = find(spec);
    if (!el) return 'no element for ' + spec;
    if (el.tagName !== 'SELECT') { el = el.querySelector('select') || el; }
    if (el.tagName !== 'SELECT') return 'not a select';
    var hit = 0;
    for (var i = 0; i < el.options.length; i++) {
      var o = el.options[i];
      var on = values.indexOf(o.value) >= 0 || values.indexOf(o.label) >= 0 || values.indexOf(o.textContent.trim()) >= 0;
      if (el.multiple) o.selected = on; else if (on) { el.selectedIndex = i; hit++; break; }
      if (on) hit++;
    }
    fire(el, 'input'); fire(el, 'change');
    return hit ? 'selected ' + hit : 'no option matched ' + JSON.stringify(values);
  }
  function fill(spec, kind, value) {
    var el = find(spec);
    if (!el) return 'no element for ' + spec;
    if (kind === 'checkbox' || kind === 'radio') {
      var want = value === 'true' || value === 'on' || value === '1' || value === 'checked';
      if (el.checked !== want) el.click();
      return 'ok';
    }
    if (kind === 'combobox' && el.tagName === 'SELECT') return select(spec, [value]);
    if (kind === 'slider') { var d = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value'); d.set.call(el, value); fire(el, 'input'); fire(el, 'change'); return 'ok'; }
    return setValue(spec, value);
  }
  function click(spec, dbl, button) {
    var el = find(spec);
    if (!el) return 'no element for ' + spec;
    el.scrollIntoView && el.scrollIntoView({ block: 'center' });
    el.focus && el.focus();
    var b = button === 'right' ? 2 : button === 'middle' ? 1 : 0;
    var r = el.getBoundingClientRect(), init = { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, button: b, buttons: 1 << b };
    fire(el, 'pointerdown', init); fire(el, 'mousedown', init); fire(el, 'pointerup', init); fire(el, 'mouseup', init);
    if (b === 2) { fire(el, 'contextmenu', init); return 'ok'; }
    if (b === 0) { el.click(); if (dbl) fire(el, 'dblclick', init); }
    return 'ok';
  }
  function hover(spec) { var el = find(spec); if (!el) return 'no element for ' + spec; var r = el.getBoundingClientRect(); var init = { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 }; fire(el, 'pointerover', init); fire(el, 'mouseover', init); fire(el, 'mouseenter', init); fire(el, 'mousemove', init); return 'ok'; }
  function focus(spec) { var el = find(spec); if (el && el.focus) el.focus(); return el ? 'ok' : 'no element'; }
  function submit(spec) { var el = find(spec); if (!el) return 'no element'; var f = el.tagName === 'FORM' ? el : el.form || el.closest('form'); if (!f) { fire(el, 'keydown', { key: 'Enter', code: 'Enter', keyCode: 13 }); fire(el, 'keyup', { key: 'Enter', code: 'Enter', keyCode: 13 }); return 'enter'; } if (f.requestSubmit) f.requestSubmit(); else f.submit(); return 'ok'; }
  function key(k) { var el = document.activeElement || document.body; var init = { key: k, code: k.length === 1 ? 'Key' + k.toUpperCase() : k, keyCode: k === 'Enter' ? 13 : k === 'Escape' ? 27 : k === 'Tab' ? 9 : k.length === 1 ? k.charCodeAt(0) : 0 }; fire(el, 'keydown', init); if (k.length === 1 && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA')) { setValue('css:' + cssPath(el), el.value + k); } fire(el, 'keyup', init); if (k === 'Enter') submit('css:' + cssPath(el)); return 'ok'; }
  function drag(a, b) {
    var from = find(a), to = find(b);
    if (!from || !to) return 'no element';
    var dt = new DataTransfer();
    var fr = from.getBoundingClientRect(), tr = to.getBoundingClientRect();
    from.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer: dt, clientX: fr.left + fr.width / 2, clientY: fr.top + fr.height / 2 }));
    to.dispatchEvent(new DragEvent('dragenter', { bubbles: true, cancelable: true, dataTransfer: dt, clientX: tr.left + tr.width / 2, clientY: tr.top + tr.height / 2 }));
    to.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt, clientX: tr.left + tr.width / 2, clientY: tr.top + tr.height / 2 }));
    to.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt, clientX: tr.left + tr.width / 2, clientY: tr.top + tr.height / 2 }));
    from.dispatchEvent(new DragEvent('dragend', { bubbles: true, cancelable: true, dataTransfer: dt }));
    return 'ok';
  }
  function scroll(spec, dx, dy) {
    var el = spec ? find(spec) : null;
    if (el) { el.scrollBy(dx, dy); return 'ok'; }
    window.scrollBy(dx, dy);
    return 'ok';
  }
  function text(spec) {
    var el = spec ? find(spec) : (document.querySelector('main, article, [role=main]') || document.body);
    if (!el) return 'no element for ' + spec;
    var t = (el.innerText || el.textContent || '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
    return t.length > 60000 ? t.slice(0, 60000) + '\n… (truncated)' : t;
  }
  function findText(needle) {
    var n = needle.toLowerCase(), out = [];
    var all = document.body.querySelectorAll('*');
    for (var i = 0; i < all.length && out.length < 40; i++) {
      var el = all[i];
      if (!visible(el)) continue;
      var own = '';
      for (var j = 0; j < el.childNodes.length; j++) if (el.childNodes[j].nodeType === 3) own += el.childNodes[j].textContent;
      if (own.toLowerCase().indexOf(n) < 0) continue;
      var target = el.closest('a, button, [role=button], [role=link], input, label, [tabindex]') || el;
      out.push('- ' + role(target) + ' ' + JSON.stringify(own.replace(/\s+/g, ' ').trim().slice(0, 100)) + ' [ref=' + ref(target) + ']');
    }
    return out.length ? out.join('\n') : 'no visible text matches ' + JSON.stringify(needle);
  }
  window.__copper = { snapshot: snapshot, find: find, rect: rect, setValue: setValue, select: select, fill: fill, click: click, hover: hover, focus: focus, submit: submit, key: key, drag: drag, scroll: scroll, text: text, findText: findText, console: function () { return logs; } };
  return true;
}
