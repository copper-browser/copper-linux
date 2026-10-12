// How tests/golden/mac-rules.json was made: the rule building of
// copper-ref Sources/Search/Shield.swift (`compile()`), copied verbatim and
// serialised the same way (JSONSerialization, no options). Run on a Mac:
//   swift mac-rules.swift > mac-rules.json
// Dictionary key order is random per run, so the test compares JSON values.
import Foundation
let unwanted = [
    "doubleclick.net", "googlesyndication.com", "googleadservices.com",
    "googletagservices.com", "google-analytics.com", "googletagmanager.com",
    "adservice.google.com", "amazon-adsystem.com", "adnxs.com", "adsrvr.org",
    "criteo.com", "criteo.net", "taboola.com", "outbrain.com",
    "rubiconproject.com", "pubmatic.com", "openx.net", "casalemedia.com",
    "smartadserver.com", "sharethrough.com", "indexww.com", "bidswitch.net",
    "33across.com", "teads.tv", "moatads.com", "adroll.com",
    "scorecardresearch.com", "quantserve.com", "chartbeat.com",
    "hotjar.com", "mouseflow.com", "fullstory.com", "clarity.ms",
    "mixpanel.com", "amplitude.com", "segment.com", "segment.io",
    "branch.io", "appsflyer.com", "adjust.com", "analytics.tiktok.com",
    "connect.facebook.net", "ads-twitter.com", "analytics.twitter.com",
]
let slots = [
    ".adsbygoogle", "ins.adsbygoogle", "[id^=\"google_ads_\"]",
    "[id^=\"div-gpt-ad\"]", "[id^=\"taboola-\"]", "#taboola-below-article",
    "iframe[src*=\"doubleclick.net\"]", "iframe[src*=\"googlesyndication\"]",
    "iframe[src*=\"amazon-adsystem\"]",
]
var rules: [[String: Any]] = unwanted.map { domain in
    let escaped = domain.replacingOccurrences(of: ".", with: "\\.")
    return [
        "trigger": [
            "url-filter": "^https?://([^/]+\\.)?\(escaped)",
            "load-type": ["third-party"],
        ],
        "action": ["type": "block"],
    ]
}
rules.append([
    "trigger": ["url-filter": ".*"],
    "action": ["type": "css-display-none", "selector": slots.joined(separator: ", ")],
])
let data = try! JSONSerialization.data(withJSONObject: rules)
FileHandle.standardOutput.write(data)
