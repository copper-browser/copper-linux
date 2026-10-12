// from Sources/Search/Fork/Canvas/CanvasFrames.swift:98 CanvasHost.frameEvent
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ e }) => {
const c = window.copperCanvas; return !!(c && typeof c.frameEvent === 'function' && c.frameEvent(e));
}
