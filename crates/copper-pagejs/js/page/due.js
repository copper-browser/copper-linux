// from Sources/Search/Fork/MCP/Tools.swift:1036 Tools.late
// How long until the page's last short timer started in the last `ms` fires (page/console_early.js); 0 without it.
({ ms }) => typeof window.__copperDue === 'function' ? window.__copperDue(ms) : 0
