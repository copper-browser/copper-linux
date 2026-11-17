// from Sources/Search/Fork/MCP/Page.swift:223 Page.screenshot
() => {
  var d = document.documentElement, b = document.body;
  return {
    x: window.scrollX, y: window.scrollY,
    width: Math.max(d.scrollWidth, b ? b.scrollWidth : 0, window.innerWidth),
    height: Math.max(d.scrollHeight, b ? b.scrollHeight : 0, window.innerHeight)
  };
}
