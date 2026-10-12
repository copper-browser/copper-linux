// from Sources/Search/Fork/Canvas/CanvasChatUI.swift:246 CanvasChat.runTool
const c = window.copperCanvas;
if (!c || typeof c.chatSend !== 'function') return JSON.stringify({ok: false, error: 'this canvas page has no chat'});
return c.chatSend(input);
