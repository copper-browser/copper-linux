//! The look: one static stylesheet, and the per-space tint written as a
//! second one whenever the space, the window's appearance or the private
//! state changes (Mac: `Design.swift` `Palette`, `Fork/SpaceTint.swift`).
//!
//! Nothing in the column has a border. Separation is spacing, a single
//! hairline, and the live row being a paper pill in a stronger shade of the
//! column's own hue.

use crate::spaces::colour::{Palette, Rgb, Theme, Tint};

/// The stylesheet that does not change.
pub const BASE: &str = r#"
window.copper { background: transparent; }
.copper-root { background-color: @copper_ground; color: @copper_ink; }

/* — the column — */
.copper-column { padding: 0; }
.copper-column .column-top { min-height: 46px; padding: 0 10px 0 10px; }
.copper-column .column-body { padding: 0 10px 8px 10px; }
.copper-column .column-foot { min-height: 44px; padding: 0 8px 0 10px; }
.copper-column scrollbar { background: transparent; }

.nav button, .copper-topbar .nav button {
  min-width: 28px; min-height: 28px; padding: 0; border-radius: 7px;
  background: transparent; box-shadow: none;
  transition: background-color 120ms ease-out, opacity 120ms ease-out;
}
.nav button:disabled { opacity: .35; }

.copper-row {
  min-height: 34px; padding: 0 6px 0 9px; border-radius: 9px;
  transition: background-color 120ms ease-out, box-shadow 120ms ease-out;
}
.copper-row .title { font-size: 14px; }
.copper-row.live .title { font-weight: 500; }
.copper-row.sleeping .title { opacity: .58; }
.copper-row .favicon { border-radius: 4px; }
.copper-row .close {
  min-width: 22px; min-height: 22px; padding: 0; border-radius: 6px;
  background: transparent; box-shadow: none; opacity: 0;
  transition: opacity 100ms ease-out, background-color 100ms ease-out;
}
.copper-row:hover .close, .copper-row:focus-visible .close { opacity: .75; }
.copper-row .close:hover { opacity: 1; }
.copper-row .badge { opacity: .6; }
.copper-row.crashed .title { opacity: .6; }
.copper-row:focus-visible { outline: 2px solid alpha(@accent_color, .6); outline-offset: -2px; }

.chip {
  min-width: 16px; min-height: 16px; border-radius: 4px;
  font-size: 9px; font-weight: 700;
}
.chip.big { min-width: 22px; min-height: 22px; border-radius: 6px; font-size: 12px; }

.new-tab-row .title { font-size: 14px; }
.new-tab-row image { opacity: .8; }

.pins { margin: 2px 0 10px 0; }
.pins > flowboxchild { padding: 0; }
.pin {
  min-height: 42px; border-radius: 11px;
  transition: background-color 120ms ease-out, box-shadow 120ms ease-out;
}
.pin:focus-visible { outline: 2px solid alpha(@accent_color, .6); outline-offset: -2px; }
.pin .letter { font-size: 13px; font-weight: 600; }

.space-head {
  min-height: 30px; padding: 0 6px 0 4px; border-radius: 8px; margin: 2px 0 4px 0;
  background: transparent; box-shadow: none;
}
.space-head .name { font-size: 13px; font-weight: 600; }
.space-head .count { font-size: 12px; }
.hairline { min-height: 1px; margin: 6px 6px; }

.space-chip {
  min-width: 26px; min-height: 26px; padding: 0; border-radius: 7px;
  background: transparent; box-shadow: none; font-size: 13px; font-weight: 600;
  transition: background-color 120ms ease-out;
}
.space-chip .dot { min-width: 7px; min-height: 7px; border-radius: 99px; }
.foot-button, menubutton.foot-button > button {
  min-width: 26px; min-height: 26px; padding: 0; border-radius: 7px;
  background: transparent; box-shadow: none;
}

/* — the header over the page (tabs in the column) — */
.copper-header { min-height: 46px; padding: 0 8px 0 12px; }
.copper-field {
  min-height: 32px; border-radius: 9px; padding: 0 12px;
  transition: background-color 120ms ease-out, box-shadow 120ms ease-out;
}
.copper-field text { font-size: 13.5px; background: transparent; }
.copper-field .private-tag, .copper-card .private-tag {
  font-size: 11px; font-weight: 600; padding: 1px 7px; border-radius: 99px;
}
.copper-field image.site { opacity: .7; }

/* — the row of tabs (tabs on top) — */
.copper-topbar { min-height: 44px; padding: 0 6px 0 8px; }
.tab-pill {
  min-height: 30px; border-radius: 9px; padding: 0 4px 0 9px;
  transition: background-color 120ms ease-out, box-shadow 120ms ease-out;
}
.tab-pill .title { font-size: 13px; }
.tab-pill.live .title { font-weight: 500; }
.tab-pill.sleeping .title { opacity: .58; }
.tab-pill.compact { padding: 0; }
.tab-pill .close {
  min-width: 20px; min-height: 20px; padding: 0; border-radius: 6px;
  background: transparent; box-shadow: none; opacity: 0;
  transition: opacity 100ms ease-out;
}
.tab-pill:hover .close { opacity: .75; }
.tab-pill:focus-visible, .top-pin:focus-visible { outline: 2px solid alpha(@accent_color, .6); outline-offset: -2px; }
.top-pin { min-width: 34px; min-height: 30px; border-radius: 9px; }
.top-space {
  min-height: 30px; padding: 0 8px; border-radius: 9px; background: transparent; box-shadow: none;
}
.top-space .name { font-size: 13px; font-weight: 600; }
.overflow-count { font-size: 12px; font-weight: 600; min-height: 30px; padding: 0 8px; border-radius: 9px; background: transparent; box-shadow: none; }

/* — the stage — */
.copper-stage { background-color: @copper_ground; }
.copper-progress { min-height: 2px; }
.copper-progress trough { min-height: 2px; background: transparent; border: none; box-shadow: none; }
.copper-progress progress { min-height: 2px; border-radius: 0 2px 2px 0; background-color: @copper_accent; border: none; }
.copper-hover {
  font-size: 12px; padding: 3px 9px; margin: 0 0 8px 8px; border-radius: 7px;
}
.copper-status { background-color: @copper_ground; }
.copper-status .title { font-size: 20px; font-weight: 600; }
.copper-status .description { opacity: .7; }
.copper-status button.pill { padding: 8px 22px; font-weight: 500; }
.copper-empty .line { font-size: 14px; }

.find-bar {
  margin: 10px 14px 0 0; padding: 4px 4px 4px 12px; border-radius: 11px;
}
.find-bar text { min-width: 200px; font-size: 13.5px; }
.find-bar .count { font-size: 12px; margin: 0 6px; }
.find-bar button { min-width: 26px; min-height: 26px; padding: 0; border-radius: 7px; background: transparent; box-shadow: none; }

/* — the card (Ctrl+T, Ctrl+K) — */
.copper-scrim.over { background-color: rgba(0,0,0,.10); }
.copper-card { border-radius: 14px; padding: 0; }
.copper-card .card-head { min-height: 54px; padding: 0 16px 0 18px; }
.copper-card .card-text { font-size: 17px; background: transparent; }
.copper-card .glass { opacity: .5; }
.copper-card .card-rule { margin: 0 12px; min-height: 1px; }
.copper-card list.copper-suggestions { padding: 6px; background: transparent; }

list.copper-suggestions { background: transparent; }
list.copper-suggestions row.copper-suggestion {
  min-height: 38px; padding: 0 10px; border-radius: 9px;
  transition: background-color 80ms ease-out;
}
row.copper-suggestion .title { font-size: 13.5px; }
row.copper-suggestion .detail { font-size: 12.5px; }
row.copper-suggestion .hint { font-size: 12px; opacity: 0; }
row.copper-suggestion:selected .hint { opacity: 1; }
row.copper-suggestion .mark { min-width: 16px; min-height: 16px; }

.copper-dropdown { padding: 6px; border-radius: 12px; }

/* — space editor — */
.swatch { min-width: 26px; min-height: 26px; padding: 0; border-radius: 99px; }
.swatch.chosen { box-shadow: 0 0 0 2px @window_bg_color, 0 0 0 4px @copper_ink; }
"#;

/// The appearance-dependent palette and every tinted surface.
pub fn tint_css(column: &Tint, private: &Tint, bar: &Tint, accent: Rgb, dark: bool) -> String {
    let p = Palette::new(dark);
    let mut css = String::new();
    css.push_str(&format!(
        "@define-color copper_ground {ground};\n@define-color copper_ink {ink};\n@define-color copper_accent {accent};\n",
        ground = p.ground.hex(),
        ink = p.ink.hex(),
        accent = accent.hex(),
    ));

    css.push_str(&surface(".copper-column", column));
    css.push_str(&surface(".copper-column.private", private));
    css.push_str(&surface(".copper-topbar", bar));
    css.push_str(&surface(".copper-topbar.private", private));

    // The header over the page, the stage, the card: the app's own neutrals.
    let card_bg = if dark { Rgb::grey(0.155) } else { Rgb::WHITE };
    let hover = if dark { Rgb::WHITE.css(0.07) } else { Rgb::BLACK.css(0.045) };
    let selected = if dark { Rgb::WHITE.css(0.10) } else { Rgb::BLACK.css(0.06) };
    css.push_str(&format!(
        r#"
.copper-header {{ background-color: {ground}; box-shadow: inset 0 -1px {hair}; }}
.copper-field {{ background-color: {wash}; color: {ink}; }}
.copper-field:hover {{ background-color: {wash_hover}; }}
.copper-field.focused {{ background-color: {card}; box-shadow: 0 0 0 1px {hair}, 0 1px 3px {shadow}; }}
.copper-field .muted {{ color: {muted}; }}
.copper-field.private {{ background-color: {pwash}; }}
.private-tag {{ background-color: {ptag}; color: {ptag_ink}; }}
.copper-header .nav button {{ color: {muted}; }}
.copper-header .nav button:hover {{ background-color: {hover}; color: {ink}; }}
.copper-stage .muted {{ color: {muted}; }}
.copper-empty .line {{ color: {muted}; }}
.copper-hover {{ background-color: {card}; color: {muted}; box-shadow: 0 0 0 1px {hair}, 0 2px 6px {shadow}; }}
.copper-card, .find-bar {{
  background-color: {card}; color: {ink};
  box-shadow: 0 0 0 1px {rim}, 0 2px 4px {shadow}, 0 18px 48px {deep};
}}
.copper-card.private {{ box-shadow: 0 0 0 1px {prim}, 0 2px 4px {shadow}, 0 18px 48px {deep}; }}
.copper-card .card-rule {{ background-color: {hair}; }}
.copper-card .card-text, .find-bar text {{ color: {ink}; caret-color: {ink}; }}
.find-bar .count {{ color: {muted}; }}
.find-bar button {{ color: {muted}; }}
.find-bar button:hover {{ background-color: {hover}; color: {ink}; }}
row.copper-suggestion {{ color: {ink}; background: transparent; }}
row.copper-suggestion:hover {{ background-color: {hover}; }}
row.copper-suggestion:selected {{ background-color: {selected}; }}
row.copper-suggestion .detail, row.copper-suggestion .hint, row.copper-suggestion .mark {{ color: {muted}; }}
.copper-dropdown {{ background-color: {card}; box-shadow: 0 0 0 1px {rim}, 0 8px 28px {deep}; }}
"#,
        ground = p.ground.hex(),
        ink = p.ink.hex(),
        muted = p.muted.hex(),
        hair = p.hairline.hex(),
        wash = p.wash.hex(),
        wash_hover = p.wash.mixed(p.ink, 0.035).hex(),
        card = card_bg.hex(),
        hover = hover,
        selected = selected,
        shadow = Rgb::BLACK.css(if dark { 0.35 } else { 0.06 }),
        deep = Rgb::BLACK.css(if dark { 0.45 } else { 0.14 }),
        rim = if dark { Rgb::WHITE.css(0.08) } else { Rgb::BLACK.css(0.07) },
        prim = private.dot().css(0.55),
        pwash = private.ground().mixed(p.ground, if dark { 0.35 } else { 0.55 }).hex(),
        ptag = private.dot().css(if dark { 0.30 } else { 0.18 }),
        ptag_ink = if dark { private.dot().mixed(Rgb::WHITE, 0.5).hex() } else { private.dot().mixed(Rgb::BLACK, 0.35).hex() },
    ));
    css
}

/// Every surface of a tinted area (`scope` is a CSS selector).
fn surface(scope: &str, t: &Tint) -> String {
    let ink = t.ink();
    format!(
        r#"
{scope} {{ background: {bg}; color: {ink}; }}
{scope} .muted {{ color: {muted}; }}
{scope} .faint {{ color: {faint}; }}
{scope} .hairline {{ background-color: {hair}; }}
{scope} .chip {{ background-color: {chip}; color: {muted}; }}
{scope} .nav button {{ color: {muted}; }}
{scope} .nav button:hover, {scope} .foot-button:hover, {scope} .space-chip:hover, {scope} .space-head:hover, {scope} .top-space:hover, {scope} .overflow-count:hover {{ background-color: {hover}; color: {ink}; }}
{scope} .nav button:active, {scope} .foot-button:active, {scope} .space-chip:active {{ background-color: {press}; }}
{scope} .foot-button, {scope} .space-chip, {scope} .overflow-count {{ color: {muted}; }}
{scope} .space-chip.current {{ background-color: {square}; color: {ink}; }}
{scope} .copper-row:hover, {scope} .tab-pill:hover, {scope} .top-pin:hover {{ background-color: {hover}; }}
{scope} .copper-row:active, {scope} .tab-pill:active {{ background-color: {press}; }}
{scope} .copper-row.live, {scope} .tab-pill.live, {scope} .top-pin.live {{
  background-color: {pill}; box-shadow: inset 0 0 0 1px {rim}, 0 1px 2px {lift};
}}
{scope} .copper-row .close, {scope} .tab-pill .close {{ color: {muted}; }}
{scope} .copper-row .close:hover, {scope} .tab-pill .close:hover {{ background-color: {hover}; color: {ink}; }}
{scope} .pin {{ background-color: {square}; }}
{scope} .pin:hover {{ background-color: {hover2}; }}
{scope} .pin.live {{ background-color: {pill}; box-shadow: inset 0 0 0 1px {rim}, 0 1px 2px {lift}; }}
{scope} .new-tab-row {{ color: {faint}; }}
{scope} .new-tab-row:hover {{ color: {muted}; background-color: {hover}; }}
{scope} .drop-before {{ box-shadow: inset 0 2px {dot}; }}
{scope} .drop-after {{ box-shadow: inset 0 -2px {dot}; }}
{scope} .drop-into {{ box-shadow: inset 0 0 0 2px {dot_soft}; }}
"#,
        bg = t.background_css(),
        ink = ink.hex(),
        muted = t.muted(),
        faint = t.faint(),
        hair = t.hairline(),
        chip = t.chip(),
        hover = t.hover(),
        hover2 = t.press(),
        press = t.press(),
        square = t.square(),
        pill = t.pill(),
        rim = t.rim(),
        lift = t.lift(),
        dot = t.dot().css(0.9),
        dot_soft = t.dot().css(0.5),
    )
}

/// The neutral tint of the tab row (tabs on top): the app's wash, not the
/// space's colour — the space shows in its chip.
pub fn bar_tint(dark: bool) -> Tint {
    Tint::new(
        Theme {
            colors: vec![Rgb::grey(0.5)],
            intensity: 0.16,
            tone: 0.0,
        },
        dark,
    )
}

/// The progress line's colour: the space's, quieter in the dark.
pub fn accent(column: &Tint) -> Rgb {
    column.dot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::session::Space;

    #[test]
    fn tint_css_has_every_surface_for_both_looks() {
        for dark in [false, true] {
            let col = Tint::of(&Space::new("Home"), dark);
            let css = tint_css(&col, &crate::private::tint(dark), &bar_tint(dark), accent(&col), dark);
            for needle in [
                ".copper-column {",
                ".copper-column.private {",
                ".copper-topbar {",
                "@define-color copper_ground",
                ".copper-row.live",
                ".pin.live",
                "row.copper-suggestion:selected",
            ] {
                assert!(css.contains(needle), "{needle} missing (dark {dark})");
            }
            assert!(!css.contains("NaN"));
        }
    }

    #[test]
    fn the_tab_row_is_quiet() {
        let light = bar_tint(false).ground();
        assert!(light.luminance() > 0.75 && light.luminance() < 0.95, "{light:?}");
        assert!(bar_tint(true).ground().luminance() < 0.05);
    }
}
