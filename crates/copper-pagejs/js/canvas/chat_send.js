// from Sources/Search/Fork/Canvas/CanvasChatUI.swift:246 CanvasChat.runTool
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ input }) => {
const c = window.copperCanvas;
if (!c || typeof c.chatSend !== 'function') return JSON.stringify({ok: false, error: 'this canvas page has no chat'});
return c.chatSend(input);
}
