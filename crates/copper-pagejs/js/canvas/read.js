// from Sources/Search/Fork/Canvas/CanvasHost.swift:801 CanvasHost.read
// callAsyncJavaScript body on the Mac; the body is verbatim, its arguments are destructured.
async ({ opts }) => {
const out = await window.copperCanvas.read(opts);
return typeof out === 'string' ? out : JSON.stringify(out);
}
