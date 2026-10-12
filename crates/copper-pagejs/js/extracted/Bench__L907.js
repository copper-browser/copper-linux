// from Sources/Search/Bench.swift:907 Bench.act
(function () {
  var el = document.querySelector(/*INTERP: sel*/);
  if (!el) return 'nothing matches ' + /*INTERP: sel*/;
  if (el.scrollIntoView) el.scrollIntoView({ block: 'center', inline: 'nearest' });
  var verb = '/*INTERP: verb*/';
  if (verb === 'click') { el.focus && el.focus(); el.click(); return 'ok'; }
  if (verb === 'submit') {
    var form = el.tagName === 'FORM' ? el : el.form || el.closest('form');
    if (!form) return 'no form around ' + /*INTERP: sel*/;
    if (form.requestSubmit) form.requestSubmit(); else form.submit();
    return 'ok';
  }
  el.focus && el.focus();
  var value = /*INTERP: txt*/;
  if (el.isContentEditable) {
    el.textContent = value;
    el.dispatchEvent(new InputEvent('input', { bubbles: true, data: value, inputType: 'insertText' }));
    return 'ok';
  }
  var proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
  var setter = Object.getOwnPropertyDescriptor(proto, 'value');
  if (setter && setter.set) setter.set.call(el, value); else el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return 'ok';
})();
