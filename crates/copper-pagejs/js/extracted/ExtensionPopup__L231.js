// from Sources/Search/ExtensionPopup.swift:231 ExtensionPopup.reach
((key) => {
  const d = document.documentElement;
  if (!d) return null;
  if (key === "height") return d.scrollHeight;
  const own = d.getBoundingClientRect().width;
  if (own > innerWidth + 1) return own;
  const saved = d.getAttribute("style");
  d.style.setProperty("width", "min-content", "important");
  const narrowest = d.getBoundingClientRect().width;
  saved === null ? d.removeAttribute("style") : d.setAttribute("style", saved);
  return narrowest >= 100 ? narrowest : Math.max(narrowest, d.scrollWidth);
})
