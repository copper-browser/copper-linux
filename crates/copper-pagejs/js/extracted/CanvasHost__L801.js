// from Sources/Search/Fork/Canvas/CanvasHost.swift:801 CanvasHost.read
const out = await window.copperCanvas.read(opts);
return typeof out === 'string' ? out : JSON.stringify(out);
