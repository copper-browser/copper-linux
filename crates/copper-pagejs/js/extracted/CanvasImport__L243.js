// from Sources/Search/Fork/Canvas/CanvasImport.swift:243 CanvasImport.migrate
const c = window.copperCanvas;
if (!c || typeof c.importLegacy !== 'function') throw new Error('this Copper\'s canvas page has no importLegacy');
const out = await c.importLegacy(payload);
return typeof out === 'string' ? out : JSON.stringify(out);
