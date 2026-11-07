//! E4: propiedades `animation-*` (CSS Animations 1 §3) y `transition-*` (CSS Transitions 1
//! §2), sus shorthands y las funciones de easing (CSS Easing 1 §2).

use std::fmt;

use bf_css::syntax::{ComponentValue, TokenKind, split_comma_separated};

use super::{LonghandId as L, PropValue, Value};
use crate::values::{components, fmt_number, ident, is_ident, parse_number, parse_time, trim};

// --- Funciones de easing ---

/// Palabra clave con la que se escribió una curva de Bézier (para serializarla igual).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BezierKeyword {
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// `<step-position>` (CSS Easing 1 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepPosition {
    JumpStart,
    JumpEnd,
    JumpNone,
    JumpBoth,
    Start,
    End,
}

/// `<easing-function>` (CSS Easing 1 §2).
#[derive(Debug, Clone, PartialEq)]
pub enum TimingFunction {
    Linear,
    CubicBezier {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        keyword: Option<BezierKeyword>,
    },
    /// `steps(n, pos)`; `start` indica `step-start`/`step-end` escritas como palabra clave
    /// (se serializan igual que `steps(1, start)`/`steps(1)`).
    Steps(u32, StepPosition),
    /// `linear(<stop>#)` (CSS Easing 2 §2.1), con las paradas ya completadas: `(entrada,
    /// salida)` con entradas no decrecientes.
    Points(Vec<(f32, f32)>),
}

impl TimingFunction {
    pub const EASE: TimingFunction = TimingFunction::CubicBezier {
        x1: 0.25,
        y1: 0.1,
        x2: 0.25,
        y2: 1.0,
        keyword: Some(BezierKeyword::Ease),
    };

    fn keyword(kw: BezierKeyword) -> TimingFunction {
        let (x1, y1, x2, y2) = match kw {
            BezierKeyword::Ease => (0.25, 0.1, 0.25, 1.0),
            BezierKeyword::EaseIn => (0.42, 0.0, 1.0, 1.0),
            BezierKeyword::EaseOut => (0.0, 0.0, 0.58, 1.0),
            BezierKeyword::EaseInOut => (0.42, 0.0, 0.58, 1.0),
        };
        TimingFunction::CubicBezier {
            x1,
            y1,
            x2,
            y2,
            keyword: Some(kw),
        }
    }

    pub fn parse(input: &[ComponentValue]) -> Option<TimingFunction> {
        let items = components(trim(input));
        let [v] = items.as_slice() else {
            return None;
        };
        if let Some(name) = ident(v) {
            return Some(match name.to_ascii_lowercase().as_str() {
                "linear" => TimingFunction::Linear,
                "ease" => Self::keyword(BezierKeyword::Ease),
                "ease-in" => Self::keyword(BezierKeyword::EaseIn),
                "ease-out" => Self::keyword(BezierKeyword::EaseOut),
                "ease-in-out" => Self::keyword(BezierKeyword::EaseInOut),
                "step-start" => TimingFunction::Steps(1, StepPosition::Start),
                "step-end" => TimingFunction::Steps(1, StepPosition::End),
                _ => return None,
            });
        }
        let ComponentValue::Function(f) = v else {
            return None;
        };
        let args = split_comma_separated(&f.value);
        let args: Vec<Vec<&ComponentValue>> = args.iter().map(|a| components(a)).collect();
        match f.name.to_ascii_lowercase().as_str() {
            "cubic-bezier" if args.len() == 4 => {
                let mut n = [0.0; 4];
                for (slot, a) in n.iter_mut().zip(&args) {
                    let [v] = a.as_slice() else {
                        return None;
                    };
                    *slot = parse_number(v)?;
                }
                // Las x deben estar en [0, 1] (§2.2).
                if !(0.0..=1.0).contains(&n[0]) || !(0.0..=1.0).contains(&n[2]) {
                    return None;
                }
                Some(TimingFunction::CubicBezier {
                    x1: n[0],
                    y1: n[1],
                    x2: n[2],
                    y2: n[3],
                    keyword: None,
                })
            }
            "steps" if (1..=2).contains(&args.len()) => {
                let [count] = args[0].as_slice() else {
                    return None;
                };
                let count = match count.token() {
                    Some(TokenKind::Number(n)) => n.as_integer()?,
                    _ => return None,
                };
                let pos = match args.get(1).map(|a| a.as_slice()) {
                    None => StepPosition::End,
                    Some([p]) => match ident(p)?.to_ascii_lowercase().as_str() {
                        "jump-start" => StepPosition::JumpStart,
                        "jump-end" => StepPosition::JumpEnd,
                        "jump-none" => StepPosition::JumpNone,
                        "jump-both" => StepPosition::JumpBoth,
                        "start" => StepPosition::Start,
                        "end" => StepPosition::End,
                        _ => return None,
                    },
                    _ => return None,
                };
                let min = if pos == StepPosition::JumpNone { 2 } else { 1 };
                if count < min {
                    return None;
                }
                Some(TimingFunction::Steps(count as u32, pos))
            }
            "linear" if !args.is_empty() => parse_linear_points(&args),
            _ => None,
        }
    }

    /// La salida para la entrada `t` (CSS Easing 1 §2). `before` es el "before flag" de las
    /// funciones escalonadas.
    pub fn eval(&self, t: f64, before: bool) -> f64 {
        match self {
            TimingFunction::Linear => t,
            TimingFunction::CubicBezier { x1, y1, x2, y2, .. } => cubic_bezier(
                f64::from(*x1),
                f64::from(*y1),
                f64::from(*x2),
                f64::from(*y2),
                t,
            ),
            TimingFunction::Steps(steps, pos) => {
                let steps = f64::from(*steps);
                let mut current = (t * steps).floor();
                if matches!(
                    pos,
                    StepPosition::JumpStart | StepPosition::Start | StepPosition::JumpBoth
                ) {
                    current += 1.0;
                }
                if before && (t * steps).fract() == 0.0 {
                    current -= 1.0;
                }
                if t >= 0.0 && current < 0.0 {
                    current = 0.0;
                }
                let jumps = match pos {
                    StepPosition::JumpNone => steps - 1.0,
                    StepPosition::JumpBoth => steps + 1.0,
                    _ => steps,
                };
                if t <= 1.0 && current > jumps {
                    current = jumps;
                }
                current / jumps
            }
            TimingFunction::Points(points) => linear_points(points, t),
        }
    }
}

/// `linear()` (CSS Easing 2 §2.1.1, "create a linear easing function").
fn parse_linear_points(args: &[Vec<&ComponentValue>]) -> Option<TimingFunction> {
    let mut stops: Vec<(f32, Option<f32>)> = Vec::new();
    for a in args {
        let pct = |v: &ComponentValue| match v.token() {
            Some(TokenKind::Percentage(p)) => Some(p.value as f32 / 100.0),
            _ => None,
        };
        match a.as_slice() {
            [o] => stops.push((parse_number(o)?, None)),
            [o, p] => stops.push((parse_number(o)?, Some(pct(p)?))),
            [o, p, q] => {
                let o = parse_number(o)?;
                stops.push((o, Some(pct(p)?)));
                stops.push((o, Some(pct(q)?)));
            }
            _ => return None,
        }
    }
    if stops.len() < 2 {
        return None;
    }
    let n = stops.len();
    if stops[0].1.is_none() {
        stops[0].1 = Some(0.0);
    }
    if stops[n - 1].1.is_none() {
        stops[n - 1].1 = Some(1.0);
    }
    // Entradas no decrecientes.
    let mut max = f32::NEG_INFINITY;
    for s in &mut stops {
        if let Some(i) = &mut s.1 {
            *i = i.max(max);
            max = *i;
        }
    }
    // Las entradas que faltan se reparten entre las conocidas.
    let mut i = 0;
    while i < n {
        if stops[i].1.is_some() {
            i += 1;
            continue;
        }
        let start = i - 1;
        let mut end = i;
        while stops[end].1.is_none() {
            end += 1;
        }
        let (a, b) = (stops[start].1?, stops[end].1?);
        let span = (end - start) as f32;
        for (k, s) in stops.iter_mut().enumerate().take(end).skip(start + 1) {
            s.1 = Some(a + (b - a) * (k - start) as f32 / span);
        }
        i = end;
    }
    Some(TimingFunction::Points(
        stops
            .into_iter()
            .map(|(o, i)| (i.unwrap_or(0.0), o))
            .collect(),
    ))
}

fn linear_points(points: &[(f32, f32)], t: f64) -> f64 {
    let p: Vec<(f64, f64)> = points
        .iter()
        .map(|&(i, o)| (f64::from(i), f64::from(o)))
        .collect();
    let n = p.len();
    // El último tramo cuya entrada inicial es <= t (o el primero / último para extrapolar).
    let seg = if t < p[0].0 {
        0
    } else {
        (0..n - 1).rev().find(|&k| p[k].0 <= t).unwrap_or(0)
    };
    let (a, b) = (p[seg], p[(seg + 1).min(n - 1)]);
    if b.0 == a.0 {
        return b.1;
    }
    a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)
}

/// Bézier cúbica con extremos (0,0) y (1,1): la `y` para la `x` = `t`. Fuera de [0, 1] se
/// extrapola con la tangente en el extremo (CSS Easing 1 §2.2).
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    if t < 0.0 {
        let slope = if x1 > 0.0 {
            y1 / x1
        } else if y1 == 0.0 && x2 > 0.0 {
            y2 / x2
        } else {
            0.0
        };
        return slope * t;
    }
    if t > 1.0 {
        let slope = if x2 < 1.0 {
            (y2 - 1.0) / (x2 - 1.0)
        } else if y2 == 1.0 && x1 < 1.0 {
            (y1 - 1.0) / (x1 - 1.0)
        } else {
            0.0
        };
        return 1.0 + slope * (t - 1.0);
    }
    let bez = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let dbez = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    // Newton y, si no converge, bisección.
    let mut s = t;
    for _ in 0..8 {
        let x = bez(x1, x2, s) - t;
        if x.abs() < 1e-7 {
            return bez(y1, y2, s);
        }
        let d = dbez(x1, x2, s);
        if d.abs() < 1e-7 {
            break;
        }
        s -= x / d;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    s = t;
    for _ in 0..60 {
        let x = bez(x1, x2, s);
        if (x - t).abs() < 1e-9 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    bez(y1, y2, s)
}

impl fmt::Display for TimingFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimingFunction::Linear => f.write_str("linear"),
            TimingFunction::CubicBezier {
                keyword: Some(k), ..
            } => f.write_str(match k {
                BezierKeyword::Ease => "ease",
                BezierKeyword::EaseIn => "ease-in",
                BezierKeyword::EaseOut => "ease-out",
                BezierKeyword::EaseInOut => "ease-in-out",
            }),
            TimingFunction::CubicBezier { x1, y1, x2, y2, .. } => write!(
                f,
                "cubic-bezier({}, {}, {}, {})",
                fmt_number(*x1),
                fmt_number(*y1),
                fmt_number(*x2),
                fmt_number(*y2)
            ),
            TimingFunction::Steps(n, pos) => match pos {
                StepPosition::End | StepPosition::JumpEnd => write!(f, "steps({n})"),
                StepPosition::Start => write!(f, "steps({n}, start)"),
                StepPosition::JumpStart => write!(f, "steps({n}, jump-start)"),
                StepPosition::JumpNone => write!(f, "steps({n}, jump-none)"),
                StepPosition::JumpBoth => write!(f, "steps({n}, jump-both)"),
            },
            TimingFunction::Points(points) => {
                let items: Vec<String> = points
                    .iter()
                    .map(|(i, o)| format!("{} {}%", fmt_number(*o), fmt_number(i * 100.0)))
                    .collect();
                write!(f, "linear({})", items.join(", "))
            }
        }
    }
}

// --- Listas separadas por comas ---

/// Un elemento de una propiedad de lista (`animation-*`, `transition-*`).
pub trait ListItem: Sized + Clone + fmt::Debug + PartialEq + Send + Sync + 'static {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self>;
    fn item_css(&self) -> String;
}

/// Una propiedad `<item>#`.
#[derive(Debug, Clone, PartialEq)]
pub struct List<T>(pub Vec<T>);

impl<T> List<T> {
    /// El elemento `i`, repitiendo la lista si es más corta (CSS Animations 1 §3).
    pub fn get(&self, i: usize) -> Option<&T> {
        if self.0.is_empty() {
            None
        } else {
            self.0.get(i % self.0.len())
        }
    }
}

impl<T: ListItem> PropValue for List<T> {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let parts = split_comma_separated(trim(input));
        let items: Vec<T> = parts
            .iter()
            .map(|p| T::parse_item(&components(p)))
            .collect::<Option<_>>()?;
        if items.is_empty() {
            return None;
        }
        Some(List(items))
    }
    fn to_css(&self) -> String {
        let items: Vec<String> = self.0.iter().map(ListItem::item_css).collect();
        items.join(", ")
    }
}

/// `<time>` en segundos (`animation-delay`, `transition-*`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Time(pub f32);

fn time_css(s: f32) -> String {
    format!("{}s", fmt_number(s))
}

impl ListItem for Time {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
        match input {
            [v] => parse_time(v).map(Time),
            _ => None,
        }
    }
    fn item_css(&self) -> String {
        time_css(self.0)
    }
}

/// `<time [0,∞]>` (duraciones). En `animation-duration` también `auto` (CSS Animations 2),
/// que vale 0 para las animaciones CSS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Duration {
    Auto,
    Secs(f32),
}

impl Duration {
    pub fn secs(self) -> f32 {
        match self {
            Duration::Auto => 0.0,
            Duration::Secs(s) => s,
        }
    }
}

impl ListItem for Duration {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
        match input {
            [v] if is_ident(v, "auto") => Some(Duration::Auto),
            [v] => parse_time(v).filter(|t| *t >= 0.0).map(Duration::Secs),
            _ => None,
        }
    }
    fn item_css(&self) -> String {
        match self {
            Duration::Auto => "auto".to_owned(),
            Duration::Secs(s) => time_css(*s),
        }
    }
}

impl ListItem for TimingFunction {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
        match input {
            [v] => TimingFunction::parse(std::slice::from_ref(*v)),
            _ => None,
        }
    }
    fn item_css(&self) -> String {
        self.to_string()
    }
}

/// `<single-animation-iteration-count>`: `infinite | <number [0,∞]>`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IterationCount(pub f32);

impl ListItem for IterationCount {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
        match input {
            [v] if is_ident(v, "infinite") => Some(IterationCount(f32::INFINITY)),
            [v] => parse_number(v).filter(|n| *n >= 0.0).map(IterationCount),
            _ => None,
        }
    }
    fn item_css(&self) -> String {
        if self.0.is_infinite() {
            "infinite".to_owned()
        } else {
            fmt_number(self.0)
        }
    }
}

macro_rules! keyword_item {
    ($name:ident { $($variant:ident = $css:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl ListItem for $name {
            fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
                let [v] = input else { return None };
                let i = ident(v)?.to_ascii_lowercase();
                $(if i == $css { return Some($name::$variant); })+
                None
            }
            fn item_css(&self) -> String {
                match self { $($name::$variant => $css.to_owned()),+ }
            }
        }
    };
}

keyword_item!(AnimationDirection {
    Normal = "normal",
    Reverse = "reverse",
    Alternate = "alternate",
    AlternateReverse = "alternate-reverse",
});

keyword_item!(FillMode {
    None = "none",
    Forwards = "forwards",
    Backwards = "backwards",
    Both = "both",
});

keyword_item!(PlayState {
    Running = "running",
    Paused = "paused",
});

keyword_item!(TransitionBehavior {
    Normal = "normal",
    AllowDiscrete = "allow-discrete",
});

/// ¿Es `name` un `<custom-ident>` válido (CSS Values 4 §4.2)?
fn custom_ident(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default"
    )
}

/// `none | <keyframes-name>` (CSS Animations 1 §3.1). Un nombre escrito como string se
/// recuerda para serializarlo con comillas.
#[derive(Debug, Clone, PartialEq)]
pub enum AnimationName {
    None,
    Ident(String),
    String(String),
}

impl AnimationName {
    pub fn name(&self) -> Option<&str> {
        match self {
            AnimationName::None => None,
            AnimationName::Ident(s) | AnimationName::String(s) => Some(s),
        }
    }
}

impl ListItem for AnimationName {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
        let [v] = input else { return None };
        match v.token()? {
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("none") => Some(AnimationName::None),
            TokenKind::Ident(s) if custom_ident(s) => Some(AnimationName::Ident(s.to_string())),
            TokenKind::String(s) => Some(AnimationName::String(s.to_string())),
            _ => None,
        }
    }
    fn item_css(&self) -> String {
        match self {
            AnimationName::None => "none".to_owned(),
            AnimationName::Ident(s) => s.clone(),
            AnimationName::String(s) => {
                // Un string que es un ident válido se serializa como ident.
                if !s.is_empty()
                    && custom_ident(s)
                    && !s.eq_ignore_ascii_case("none")
                    && s.chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                    && !s.starts_with(|c: char| c.is_ascii_digit())
                {
                    s.clone()
                } else {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                }
            }
        }
    }
}

/// `none | <single-transition-property>` (CSS Transitions 1 §2.1).
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionProperty {
    None,
    All,
    Name(String),
}

impl ListItem for TransitionProperty {
    fn parse_item(input: &[&ComponentValue]) -> Option<Self> {
        let [v] = input else { return None };
        let i = ident(v)?;
        Some(match i.to_ascii_lowercase().as_str() {
            "none" => TransitionProperty::None,
            "all" => TransitionProperty::All,
            _ if custom_ident(i) => TransitionProperty::Name(if i.starts_with("--") {
                i.to_owned()
            } else {
                i.to_ascii_lowercase()
            }),
            _ => return None,
        })
    }
    fn item_css(&self) -> String {
        match self {
            TransitionProperty::None => "none".to_owned(),
            TransitionProperty::All => "all".to_owned(),
            TransitionProperty::Name(n) => n.clone(),
        }
    }
}

pub type AnimationNames = List<AnimationName>;
pub type Durations = List<Duration>;
pub type Times = List<Time>;
pub type TimingFunctions = List<TimingFunction>;
pub type IterationCounts = List<IterationCount>;
pub type AnimationDirections = List<AnimationDirection>;
pub type FillModes = List<FillMode>;
pub type PlayStates = List<PlayState>;
pub type TransitionProperties = List<TransitionProperty>;
pub type TransitionBehaviors = List<TransitionBehavior>;

/// `transition-property` con `none` solo como único valor (CSS Transitions 1 §2.1).
pub fn transition_property_valid(list: &TransitionProperties) -> bool {
    list.0.len() == 1 || !list.0.contains(&TransitionProperty::None)
}

// --- Shorthands ---

pub const ANIMATION: &[L] = &[
    L::AnimationDuration,
    L::AnimationTimingFunction,
    L::AnimationDelay,
    L::AnimationIterationCount,
    L::AnimationDirection,
    L::AnimationFillMode,
    L::AnimationPlayState,
    L::AnimationName,
];

pub const TRANSITION: &[L] = &[
    L::TransitionProperty,
    L::TransitionDuration,
    L::TransitionTimingFunction,
    L::TransitionDelay,
    L::TransitionBehavior,
];

/// Un `<single-animation>` (CSS Animations 1 §3.11): `<time> || <easing-function> || <time>
/// || <iteration-count> || <direction> || <fill-mode> || <play-state> || [none |
/// <keyframes-name>]`.
#[allow(clippy::type_complexity)]
fn single_animation(
    items: &[&ComponentValue],
) -> Option<(
    Duration,
    TimingFunction,
    Time,
    IterationCount,
    AnimationDirection,
    FillMode,
    PlayState,
    AnimationName,
)> {
    let mut duration = None;
    let mut easing = None;
    let mut delay = None;
    let mut count = None;
    let mut direction = None;
    let mut fill = None;
    let mut play = None;
    let mut name = None;
    for v in items {
        let one = std::slice::from_ref(v);
        if let Some(t) = parse_time(v) {
            if duration.is_none() {
                if t < 0.0 {
                    return None;
                }
                duration = Some(Duration::Secs(t));
            } else if delay.is_none() {
                delay = Some(Time(t));
            } else {
                return None;
            }
        } else if easing.is_none()
            && let Some(e) = TimingFunction::parse_item(one)
        {
            easing = Some(e);
        } else if count.is_none()
            && let Some(c) = IterationCount::parse_item(one)
        {
            count = Some(c);
        } else if direction.is_none()
            && let Some(d) = AnimationDirection::parse_item(one)
        {
            direction = Some(d);
        } else if fill.is_none()
            && let Some(f) = FillMode::parse_item(one)
        {
            fill = Some(f);
        } else if play.is_none()
            && let Some(p) = PlayState::parse_item(one)
        {
            play = Some(p);
        } else if name.is_none()
            && let Some(n) = AnimationName::parse_item(one)
        {
            name = Some(n);
        } else {
            return None;
        }
    }
    Some((
        duration.unwrap_or(Duration::Secs(0.0)),
        easing.unwrap_or(TimingFunction::EASE),
        delay.unwrap_or(Time(0.0)),
        count.unwrap_or(IterationCount(1.0)),
        direction.unwrap_or(AnimationDirection::Normal),
        fill.unwrap_or(FillMode::None),
        play.unwrap_or(PlayState::Running),
        name.unwrap_or(AnimationName::None),
    ))
}

/// Expande `animation`.
pub fn expand_animation(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let parts = split_comma_separated(trim(input));
    let mut lists = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for p in &parts {
        let items = components(p);
        if items.is_empty() {
            return None;
        }
        let (d, e, t, c, dir, f, pl, n) = single_animation(&items)?;
        lists.0.push(d);
        lists.1.push(e);
        lists.2.push(t);
        lists.3.push(c);
        lists.4.push(dir);
        lists.5.push(f);
        lists.6.push(pl);
        lists.7.push(n);
    }
    Some(vec![
        Value::AnimationDuration(List(lists.0)),
        Value::AnimationTimingFunction(List(lists.1)),
        Value::AnimationDelay(List(lists.2)),
        Value::AnimationIterationCount(List(lists.3)),
        Value::AnimationDirection(List(lists.4)),
        Value::AnimationFillMode(List(lists.5)),
        Value::AnimationPlayState(List(lists.6)),
        Value::AnimationName(List(lists.7)),
    ])
}

/// Expande `transition` (CSS Transitions 1 §2.5): `[none | <single-transition-property>] ||
/// <time> || <easing-function> || <time> || <transition-behavior-value>`. `none` solo vale
/// en la única transición.
pub fn expand_transition(input: &[ComponentValue]) -> Option<Vec<Value>> {
    let parts = split_comma_separated(trim(input));
    let (mut props, mut durs, mut eases, mut delays, mut behaviors) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let count = parts.len();
    for p in &parts {
        let items = components(p);
        if items.is_empty() {
            return None;
        }
        let (mut prop, mut dur, mut ease, mut delay, mut behavior) = (None, None, None, None, None);
        for v in &items {
            let one = std::slice::from_ref(v);
            if let Some(t) = parse_time(v) {
                if dur.is_none() {
                    if t < 0.0 {
                        return None;
                    }
                    dur = Some(Duration::Secs(t));
                } else if delay.is_none() {
                    delay = Some(Time(t));
                } else {
                    return None;
                }
            } else if ease.is_none()
                && let Some(e) = TimingFunction::parse_item(one)
            {
                ease = Some(e);
            } else if behavior.is_none()
                && let Some(b) = TransitionBehavior::parse_item(one)
            {
                behavior = Some(b);
            } else if prop.is_none()
                && let Some(p) = TransitionProperty::parse_item(one)
            {
                if p == TransitionProperty::None && count > 1 {
                    return None;
                }
                prop = Some(p);
            } else {
                return None;
            }
        }
        props.push(prop.unwrap_or(TransitionProperty::All));
        durs.push(dur.unwrap_or(Duration::Secs(0.0)));
        eases.push(ease.unwrap_or(TimingFunction::EASE));
        delays.push(delay.unwrap_or(Time(0.0)));
        behaviors.push(behavior.unwrap_or(TransitionBehavior::Normal));
    }
    Some(vec![
        Value::TransitionProperty(List(props)),
        Value::TransitionDuration(List(durs)),
        Value::TransitionTimingFunction(List(eases)),
        Value::TransitionDelay(List(delays)),
        Value::TransitionBehavior(List(behaviors)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(css: &str) -> Option<TimingFunction> {
        TimingFunction::parse(&bf_css::syntax::parse_component_value_list(css))
    }

    #[test]
    fn easing_functions() {
        let ease = tf("ease").unwrap();
        assert_eq!(ease.to_string(), "ease");
        assert!((ease.eval(0.5, false) - 0.8024).abs() < 1e-3);
        assert_eq!(tf("linear").unwrap().eval(0.3, false), 0.3);
        let s = tf("steps(4)").unwrap();
        assert_eq!(s.eval(0.3, false), 0.25);
        assert_eq!(tf("step-start").unwrap().to_string(), "steps(1, start)");
        assert_eq!(tf("step-start").unwrap().eval(0.0, false), 1.0);
        assert_eq!(tf("steps(2, jump-none)").unwrap().eval(0.6, false), 1.0);
        assert!(tf("cubic-bezier(2, 0, 0, 1)").is_none());
        assert!(tf("steps(0)").is_none());
        let l = tf("linear(0, 0.25 75%, 1)").unwrap();
        assert!((l.eval(0.375, false) - 0.125).abs() < 1e-6);
    }
}
