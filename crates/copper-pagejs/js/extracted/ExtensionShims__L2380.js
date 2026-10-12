// from Sources/Search/ExtensionShims.swift:2380 ExtensionShims.userScriptFile
/* Search: a user script (chrome.userScripts) */
search_user_script: {
  const __searchHref = location.href;
  const __searchGlob = (g) => new RegExp("^" + g.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$");
  const __searchIn = /*INTERP: json(script["includeGlobs"] ?? [])*/, __searchOut = /*INTERP: json(script["excludeGlobs"] ?? [])*/;
  if ((__searchIn.length && !__searchIn.some((g) => __searchGlob(g).test(__searchHref))) || __searchOut.some((g) => __searchGlob(g).test(__searchHref))) break search_user_script;
/*INTERP: userWorld ? prelude : ""*/
/*INTERP: code*/
}
