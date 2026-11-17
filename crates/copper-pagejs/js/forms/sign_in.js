// from Sources/Search/Tab.swift:847 Tab.signIn
// Secret redaction: an agent's sign-in, 'ok' or why nothing was left on the page.
({ user, password }) => (window.__officeForms && window.__officeForms.signIn && window.__officeForms.signIn(user, password)) || 'no-fields'
