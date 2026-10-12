// from Sources/Search/Fork/Canvas/CanvasChatUI.swift:327 CanvasChat.bench
const t = document.querySelector('[data-testid=canvas-chat] textarea'); if (!t) return 'no composer';
t.dispatchEvent(new KeyboardEvent('keydown', {key: key, bubbles: true, cancelable: true}));
return 'ok';
