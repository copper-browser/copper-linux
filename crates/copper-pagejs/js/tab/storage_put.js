// from Sources/Search/Fork/StorageImport.swift:169 StorageImport.put
// callAsyncJavaScript body on the Mac with argument items; the body is verbatim.
({ items }) => {
const failed = [];
for (const [k, v] of Object.entries(items)) {
    try { localStorage.setItem(k, v) } catch (e) { failed.push(k + ": " + e.name) }
}
return failed;
}
