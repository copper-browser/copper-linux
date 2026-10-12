// from Sources/Search/Fork/Canvas/CanvasChatUI.swift:316 CanvasChat.bench
const t = document.querySelector('[data-testid=canvas-chat] textarea'); if (!t) return 'no composer';
t.focus();
const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
set.call(t, t.value + text);
t.setSelectionRange(t.value.length, t.value.length);
t.dispatchEvent(new Event('input', {bubbles: true}));
t.dispatchEvent(new Event('select', {bubbles: true}));
return 'ok';
