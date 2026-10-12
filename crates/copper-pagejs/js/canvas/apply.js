// from Sources/Search/Fork/Canvas/CanvasHost.swift:788 CanvasHost.apply
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ input }) => {
const out = await window.copperCanvas.apply(input);
return typeof out === 'string' ? out : JSON.stringify(out);
}
