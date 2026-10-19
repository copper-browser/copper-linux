//! A space's colours: the named palette, the theme the column is washed in,
//! and every surface drawn over it (Mac: `Fork/Spaces.swift` `SpaceColour`,
//! `Fork/SpaceTheme.swift`, `Fork/SpaceTint.swift`).
//!
//! No GTK here: everything comes out as [`Rgb`] / CSS strings, so the maths
//! is unit-tested and the shell only pastes the results into a stylesheet.

use copper_core::session::{ColorStop, Space, SpaceTheme};

/// An sRGB colour, each channel 0…1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const WHITE: Rgb = Rgb::new(1.0, 1.0, 1.0);
    pub const BLACK: Rgb = Rgb::new(0.0, 0.0, 0.0);

    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Rgb { r, g, b }
    }

    fn clamped(self) -> Self {
        Rgb::new(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
        )
    }

    /// A grey of `white` brightness.
    pub const fn grey(white: f64) -> Self {
        Rgb::new(white, white, white)
    }

    /// From hue / saturation / brightness (HSV, `NSColor(hue:saturation:brightness:)`).
    pub fn hsv(hue: f64, saturation: f64, value: f64) -> Self {
        let h = (hue - hue.floor()) * 6.0;
        let s = saturation.clamp(0.0, 1.0);
        let v = value.clamp(0.0, 1.0);
        let i = h.floor();
        let f = h - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i as i32 % 6 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Rgb::new(r, g, b)
    }

    /// Hue, saturation, brightness, each 0…1.
    pub fn to_hsv(self) -> (f64, f64, f64) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let d = max - min;
        let s = if max <= 0.0 { 0.0 } else { d / max };
        let h = if d <= f64::EPSILON {
            0.0
        } else if max == self.r {
            ((self.g - self.b) / d).rem_euclid(6.0) / 6.0
        } else if max == self.g {
            ((self.b - self.r) / d + 2.0) / 6.0
        } else {
            ((self.r - self.g) / d + 4.0) / 6.0
        };
        (h, s, max)
    }

    /// `t` of the way to `other`.
    pub fn mixed(self, other: Rgb, t: f64) -> Rgb {
        Rgb::new(
            self.r + (other.r - self.r) * t,
            self.g + (other.g - self.g) * t,
            self.b + (other.b - self.b) * t,
        )
        .clamped()
    }

    pub fn scaled(self, k: f64) -> Rgb {
        Rgb::new(self.r * k, self.g * k, self.b * k).clamped()
    }

    /// Relative luminance, 0 (black) … 1 (white).
    pub fn luminance(self) -> f64 {
        fn linear(c: f64) -> f64 {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    fn byte(c: f64) -> u8 {
        (c.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// `#rrggbb`.
    pub fn hex(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            Self::byte(self.r),
            Self::byte(self.g),
            Self::byte(self.b)
        )
    }

    /// `rgba(r,g,b,a)` for a stylesheet.
    pub fn css(self, alpha: f64) -> String {
        format!(
            "rgba({},{},{},{:.3})",
            Self::byte(self.r),
            Self::byte(self.g),
            Self::byte(self.b),
            alpha.clamp(0.0, 1.0)
        )
    }

    fn from_stop(s: &ColorStop) -> Self {
        Rgb::new(s.r, s.g, s.b).clamped()
    }

    fn to_stop(self) -> ColorStop {
        ColorStop {
            r: self.r,
            g: self.g,
            b: self.b,
            ..Default::default()
        }
    }
}

/// The ten names a space's colour goes by (`SpaceColour`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpaceColour {
    Graphite,
    Copper,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Indigo,
    Pink,
    Red,
}

impl SpaceColour {
    pub const ALL: [SpaceColour; 10] = [
        SpaceColour::Graphite,
        SpaceColour::Copper,
        SpaceColour::Orange,
        SpaceColour::Yellow,
        SpaceColour::Green,
        SpaceColour::Teal,
        SpaceColour::Blue,
        SpaceColour::Indigo,
        SpaceColour::Pink,
        SpaceColour::Red,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SpaceColour::Graphite => "Graphite",
            SpaceColour::Copper => "Copper",
            SpaceColour::Orange => "Orange",
            SpaceColour::Yellow => "Yellow",
            SpaceColour::Green => "Green",
            SpaceColour::Teal => "Teal",
            SpaceColour::Blue => "Blue",
            SpaceColour::Indigo => "Indigo",
            SpaceColour::Pink => "Pink",
            SpaceColour::Red => "Red",
        }
    }

    /// The stored hue; `None` is grey.
    pub fn hue(self) -> Option<f64> {
        match self {
            SpaceColour::Graphite => None,
            SpaceColour::Copper => Some(0.07),
            SpaceColour::Orange => Some(0.10),
            SpaceColour::Yellow => Some(0.14),
            SpaceColour::Green => Some(0.36),
            SpaceColour::Teal => Some(0.48),
            SpaceColour::Blue => Some(0.60),
            SpaceColour::Indigo => Some(0.70),
            SpaceColour::Pink => Some(0.90),
            SpaceColour::Red => Some(0.99),
        }
    }

    pub fn parse(name: &str) -> Option<SpaceColour> {
        Self::ALL
            .into_iter()
            .find(|c| c.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The name a hue is nearest to — an imported 0.33 is Green.
    pub fn nearest(hue: Option<f64>) -> SpaceColour {
        let Some(hue) = hue else {
            return SpaceColour::Graphite;
        };
        let distance = |a: f64| {
            let d = (a - hue).abs();
            d.min(1.0 - d)
        };
        Self::ALL
            .into_iter()
            .filter(|c| *c != SpaceColour::Graphite)
            .min_by(|a, b| {
                distance(a.hue().unwrap_or(0.0)).total_cmp(&distance(b.hue().unwrap_or(0.0)))
            })
            .unwrap_or(SpaceColour::Graphite)
    }

    /// The first real colour no space wears; when all are taken, the one
    /// worn least. Never grey — grey is a choice, not a default.
    pub fn unused<'a>(spaces: impl IntoIterator<Item = &'a Space>) -> SpaceColour {
        let worn: Vec<SpaceColour> = spaces.into_iter().map(SpaceColour::of).collect();
        let colours = Self::ALL
            .into_iter()
            .filter(|c| *c != SpaceColour::Graphite);
        if let Some(free) = colours.clone().find(|c| !worn.contains(c)) {
            return free;
        }
        colours
            .min_by_key(|c| worn.iter().filter(|w| *w == c).count())
            .unwrap_or(SpaceColour::Copper)
    }

    /// What a space is called by colour.
    pub fn of(space: &Space) -> SpaceColour {
        match space.hue {
            Some(h) => SpaceColour::nearest(Some(h)),
            None if space.theme.is_some() => SpaceColour::Graphite,
            // A space never given a colour wears the copper default.
            None => SpaceColour::Copper,
        }
    }

    /// Apply to a space the way the Mac's `Spaces.tint` does: the hue
    /// alone; Graphite is the plain grey theme.
    pub fn apply(self, space: &mut Space) {
        space.hue = self.hue();
        space.theme = match self {
            SpaceColour::Graphite => Some(Theme::plain().to_space_theme()),
            _ => None,
        };
    }
}

/// A space's look, as the column draws it (`SpaceTheme`, minus the picture
/// and the animated backdrop, which Linux v1 does not draw).
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub colors: Vec<Rgb>,
    pub intensity: f64,
    pub tone: f64,
}

/// Around the wheel: how full and how bright a bare hue is taken.
const STRENGTHS: [(f64, f64, f64); 12] = [
    (0.00, 0.74, 1.00),
    (0.07, 0.74, 1.00),
    (0.10, 0.72, 1.00),
    (0.14, 0.66, 1.00),
    (0.22, 0.56, 0.94),
    (0.36, 0.50, 0.86),
    (0.48, 0.52, 0.84),
    (0.60, 0.62, 0.98),
    (0.70, 0.58, 0.98),
    (0.80, 0.54, 0.98),
    (0.90, 0.62, 1.00),
    (1.00, 0.74, 1.00),
];

/// How far the tone's black or white reaches.
const TONE_REACH: f64 = 0.7;

impl Theme {
    /// Graphite.
    pub fn plain() -> Theme {
        Theme {
            colors: vec![Rgb::new(0.62, 0.62, 0.64)],
            intensity: 0.34,
            tone: 0.0,
        }
    }

    /// The copper default (its picture is not drawn on Linux; the pale
    /// copper colour under it is).
    pub fn copper() -> Theme {
        Theme {
            colors: vec![Rgb::new(0.90, 0.76, 0.67)],
            intensity: 0.45,
            tone: 0.0,
        }
    }

    /// A private tab's column: a quiet violet-grey, the same in every space.
    pub fn private() -> Theme {
        Theme {
            colors: vec![Rgb::new(0.40, 0.36, 0.60)],
            intensity: 0.38,
            tone: 0.0,
        }
    }

    /// A bare hue at the strength that hue can carry.
    pub fn hue(hue: f64) -> Theme {
        let (s, v) = strength(hue);
        Theme {
            colors: vec![Rgb::hsv(hue, s, v)],
            intensity: 0.8,
            tone: 0.0,
        }
    }

    /// `Space.look`: its theme, else its hue, else copper.
    pub fn of(space: &Space) -> Theme {
        if let Some(t) = &space.theme {
            return Theme::from_space_theme(t);
        }
        match space.hue {
            Some(h) => Theme::hue(h),
            None => Theme::copper(),
        }
    }

    pub fn from_space_theme(t: &SpaceTheme) -> Theme {
        let mut colors: Vec<Rgb> = t.colors.iter().take(3).map(Rgb::from_stop).collect();
        if colors.is_empty() {
            colors = Theme::plain().colors;
        }
        Theme {
            colors,
            intensity: t.intensity.unwrap_or(0.8).clamp(0.0, 1.0),
            tone: t.tone.unwrap_or(0.0).clamp(-1.0, 1.0),
        }
    }

    pub fn to_space_theme(&self) -> SpaceTheme {
        SpaceTheme {
            colors: self.colors.iter().map(|c| c.to_stop()).collect(),
            intensity: Some(self.intensity),
            ..Default::default()
        }
    }

    /// The hue the rest of the app knows the space by; `None` when no stop
    /// is coloured enough to name.
    pub fn hue_of(&self) -> Option<f64> {
        let (h, s, _) = self
            .colors
            .iter()
            .map(|c| c.to_hsv())
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        (s > 0.12).then_some(h)
    }

    /// The column's colour for one stop.
    pub fn ground(&self, stop: Rgb, dark: bool) -> Rgb {
        if dark {
            let (h, s, _) = stop.to_hsv();
            return Rgb::hsv(h, (s * 0.8).min(1.0), 0.17 + 0.09 * self.intensity);
        }
        Rgb::WHITE.mixed(stop, 0.8 * self.intensity).scaled(0.96)
    }

    /// One colour standing for the whole column, tone included — what the
    /// ink has to read on.
    pub fn toned(&self, dark: bool) -> Rgb {
        let stops = if self.colors.is_empty() {
            Theme::plain().colors
        } else {
            self.colors.clone()
        };
        let sum = if stops.len() == 1 {
            self.ground(stops[0], dark)
        } else {
            let n = (stops.len() - 1) as f64;
            let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
            for (i, s) in stops.iter().enumerate() {
                let w = if i == 0 || i == stops.len() - 1 {
                    0.5
                } else {
                    1.0
                } / n;
                r += s.r * w;
                g += s.g * w;
                b += s.b * w;
            }
            self.ground(Rgb::new(r, g, b), dark)
        };
        if self.tone.abs() <= 0.005 {
            return sum;
        }
        let toward = if self.tone < 0.0 {
            Rgb::BLACK
        } else {
            Rgb::WHITE
        };
        sum.mixed(toward, self.tone.abs() * TONE_REACH)
    }

    /// The column's background as CSS: a colour, or a gradient running top
    /// leading to bottom trailing, with the tone laid over it.
    pub fn css_background(&self, dark: bool) -> String {
        let tone = |c: Rgb| {
            if self.tone.abs() <= 0.005 {
                c
            } else {
                let toward = if self.tone < 0.0 {
                    Rgb::BLACK
                } else {
                    Rgb::WHITE
                };
                c.mixed(toward, self.tone.abs() * TONE_REACH)
            }
        };
        if self.colors.len() < 2 {
            return self.toned(dark).hex();
        }
        let stops: Vec<String> = self
            .colors
            .iter()
            .map(|c| tone(self.ground(*c, dark)).hex())
            .collect();
        format!("linear-gradient(160deg, {})", stops.join(", "))
    }
}

/// `SpaceTheme.strength`: saturation and brightness for a bare hue.
pub fn strength(hue: f64) -> (f64, f64) {
    let h = hue - hue.floor();
    for pair in STRENGTHS.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if h <= b.0 {
            let t = if b.0 > a.0 {
                (h - a.0) / (b.0 - a.0)
            } else {
                0.0
            };
            return (a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t);
        }
    }
    (0.74, 1.0)
}

/// Every surface the column draws (`SpaceTint`).
#[derive(Debug, Clone, PartialEq)]
pub struct Tint {
    /// The hue the space is known by, `None` for grey.
    pub hue: Option<f64>,
    /// The column, as drawn, is dark: light ink, white translucent surfaces.
    pub dark: bool,
    /// The window is in dark mode.
    pub night: bool,
    pub theme: Theme,
}

/// Luminance where light and dark ink read equally well.
const TURN: f64 = 0.2;

impl Tint {
    pub fn new(theme: Theme, night: bool) -> Tint {
        let dark = theme.toned(night).luminance() < TURN;
        Tint {
            hue: theme.hue_of(),
            dark,
            night,
            theme,
        }
    }

    pub fn of(space: &Space, night: bool) -> Tint {
        Tint::new(Theme::of(space), night)
    }

    /// The same space on the app's own ground (a card, a menu swatch).
    pub fn off_column(&self) -> Tint {
        Tint {
            dark: self.night,
            ..self.clone()
        }
    }

    fn mix(&self, saturation: f64, brightness: f64) -> Rgb {
        match self.hue {
            Some(h) => Rgb::hsv(h, saturation, brightness),
            None => Rgb::grey(brightness),
        }
    }

    /// The column's flat colour.
    pub fn ground(&self) -> Rgb {
        self.theme.toned(self.night)
    }

    /// The ground as CSS (colour or gradient).
    pub fn background_css(&self) -> String {
        self.theme.css_background(self.night)
    }

    fn veil(&self, white: f64, black: f64) -> String {
        if self.dark {
            Rgb::WHITE.css(white)
        } else {
            Rgb::BLACK.css(black)
        }
    }

    /// A favourite at rest, the address field.
    pub fn square(&self) -> String {
        self.veil(0.07, 0.045)
    }
    /// Under the pointer.
    pub fn hover(&self) -> String {
        self.veil(0.09, 0.06)
    }
    /// Pressed.
    pub fn press(&self) -> String {
        self.veil(0.12, 0.085)
    }
    /// The live row: paper laid on the column.
    pub fn pill(&self) -> String {
        Rgb::WHITE.css(if self.dark { 0.16 } else { 0.5 })
    }
    /// The live pill's edge.
    pub fn rim(&self) -> String {
        Rgb::WHITE.css(if self.dark { 0.14 } else { 0.62 })
    }
    /// The live pill's shadow.
    pub fn lift(&self) -> String {
        Rgb::BLACK.css(if self.dark { 0.32 } else { 0.07 })
    }
    /// The only line in the column.
    pub fn hairline(&self) -> String {
        self.veil(0.10, 0.08)
    }
    /// The square a letter sits on.
    pub fn chip(&self) -> String {
        self.veil(0.10, 0.07)
    }
    /// The space's colour at full strength.
    pub fn dot(&self) -> Rgb {
        self.mix(0.62, if self.night { 0.86 } else { 0.70 })
    }
    /// The colour as a swatch shows it.
    pub fn swatch(&self) -> Rgb {
        let Some(h) = self.hue else {
            return Rgb::grey(if self.night { 0.55 } else { 0.64 });
        };
        let (s, v) = strength(h);
        Rgb::hsv(
            h,
            (s + 0.14).min(1.0),
            v * if self.night { 0.92 } else { 0.9 },
        )
    }
    /// A space's mark (letter or symbol) on a chip.
    pub fn mark(&self) -> Rgb {
        self.mix(
            if self.hue.is_none() { 0.0 } else { 0.66 },
            if self.dark { 0.92 } else { 0.40 },
        )
    }
    /// Titles: the hue taken nearly to black (to white on a dark column).
    pub fn ink(&self) -> Rgb {
        if self.dark {
            self.mix(0.06, 0.96)
        } else {
            self.mix(0.9, 0.19)
        }
    }
    pub fn muted(&self) -> String {
        self.ink().css(0.62)
    }
    pub fn faint(&self) -> String {
        self.ink().css(0.45)
    }
}

/// The app's own neutrals (`Palette`): one value for light, one for dark.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub ground: Rgb,
    pub ink: Rgb,
    pub muted: Rgb,
    pub faint: Rgb,
    pub hairline: Rgb,
    pub wash: Rgb,
    pub hover: Rgb,
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        let pair = |l: f64, d: f64| Rgb::grey(if dark { d } else { l });
        Palette {
            ground: pair(1.0, 0.11),
            ink: pair(0.09, 0.93),
            muted: pair(0.55, 0.58),
            faint: pair(0.83, 0.32),
            hairline: pair(0.91, 0.20),
            wash: pair(0.937, 0.175),
            hover: pair(0.965, 0.15),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn hsv_round_trip() {
        for (h, s, v) in [
            (0.0, 1.0, 1.0),
            (0.6, 0.62, 0.98),
            (0.36, 0.5, 0.86),
            (0.9, 0.2, 0.3),
        ] {
            let c = Rgb::hsv(h, s, v);
            let (h2, s2, v2) = c.to_hsv();
            assert!(
                close(h, h2) && close(s, s2) && close(v, v2),
                "{h} {s} {v} → {c:?}"
            );
        }
        assert_eq!(Rgb::hsv(0.0, 1.0, 1.0).hex(), "#ff0000");
        assert_eq!(Rgb::hsv(1.0 / 3.0, 1.0, 1.0).hex(), "#00ff00");
    }

    #[test]
    fn strengths_interpolate() {
        assert_eq!(strength(0.07), (0.74, 1.0));
        let (s, v) = strength(0.54);
        assert!(s > 0.52 && s < 0.62 && v > 0.84 && v < 0.98);
        assert_eq!(strength(1.07), strength(0.07));
    }

    #[test]
    fn copper_default_is_a_pale_warm_column_with_dark_ink() {
        let tint = Tint::of(&Space::new("Home"), false);
        let g = tint.ground();
        assert!(g.r > g.g && g.g > g.b, "warm: {g:?}");
        assert!(g.luminance() > 0.6, "pale");
        assert!(!tint.dark);
        assert!(tint.ink().luminance() < 0.05);
        let night = Tint::of(&Space::new("Home"), true);
        assert!(night.dark, "dark mode columns are dark");
        assert!(night.ink().luminance() > 0.8);
    }

    #[test]
    fn named_colours() {
        assert_eq!(SpaceColour::nearest(Some(0.33)), SpaceColour::Green);
        assert_eq!(SpaceColour::nearest(Some(0.02)), SpaceColour::Red);
        assert_eq!(SpaceColour::nearest(None), SpaceColour::Graphite);
        assert_eq!(SpaceColour::parse("teal"), Some(SpaceColour::Teal));
        let mut a = Space::new("A");
        SpaceColour::Copper.apply(&mut a);
        let mut b = Space::new("B");
        SpaceColour::Orange.apply(&mut b);
        assert_eq!(SpaceColour::unused([&a, &b]), SpaceColour::Yellow);
        let mut g = Space::new("G");
        SpaceColour::Graphite.apply(&mut g);
        assert_eq!(SpaceColour::of(&g), SpaceColour::Graphite);
        assert_eq!(Tint::of(&g, false).hue, None);
        assert_eq!(SpaceColour::of(&Space::new("fresh")), SpaceColour::Copper);
    }

    #[test]
    fn gradients_and_tone() {
        let t = Theme {
            colors: vec![Rgb::new(1.0, 0.5, 0.4), Rgb::new(0.4, 0.5, 1.0)],
            intensity: 0.8,
            tone: 0.0,
        };
        assert!(t.css_background(false).starts_with("linear-gradient("));
        let mut darker = t.clone();
        darker.tone = -1.0;
        assert!(darker.toned(false).luminance() < t.toned(false).luminance());
        assert!(Tint::new(darker, false).dark, "toned to black: light ink");
    }

    #[test]
    fn theme_round_trips_through_the_session_model() {
        let plain = Theme::plain();
        assert_eq!(Theme::from_space_theme(&plain.to_space_theme()), plain);
    }
}
