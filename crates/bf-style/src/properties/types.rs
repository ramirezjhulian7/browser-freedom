//! Tipos de valor de las propiedades. Cada tipo implementa [`PropValue`]: parseo desde
//! component values (ya sin whitespace en los extremos), cómputo y serialización para volcados.
//! El mismo tipo representa el valor especificado y el computado.

use std::fmt::Write as _;

use bf_css::syntax::{ComponentValue, TokenKind, split_comma_separated};

use super::{ComputeContext, PropValue};
use crate::values::color::parse_color;
use crate::values::{
    Color, LengthPercentage, Rgba, components, fmt_number, ident, integer, is_ident, parse_length,
    parse_length_percentage, parse_non_negative_lp, parse_number, parse_url, single_ident, trim,
};

/// Define un enum de keywords con su `PropValue`.
macro_rules! keyword_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident = $css:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn from_ident(s: &str) -> Option<Self> {
                $(if s.eq_ignore_ascii_case($css) { return Some($name::$variant); })+
                None
            }

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $css),+ }
            }
        }
    };
}

/// `PropValue` estándar para un enum de keywords.
macro_rules! keyword_prop {
    ($name:ident) => {
        impl PropValue for $name {
            fn parse(input: &[ComponentValue]) -> Option<Self> {
                single_ident(input).and_then(Self::from_ident)
            }
            fn to_css(&self) -> String {
                self.as_str().to_owned()
            }
        }
    };
}

keyword_enum!(Display {
    None = "none",
    Contents = "contents",
    Block = "block",
    Inline = "inline",
    InlineBlock = "inline-block",
    ListItem = "list-item",
    FlowRoot = "flow-root",
    Flex = "flex",
    InlineFlex = "inline-flex",
    Grid = "grid",
    InlineGrid = "inline-grid",
    Table = "table",
    InlineTable = "inline-table",
    TableRowGroup = "table-row-group",
    TableHeaderGroup = "table-header-group",
    TableFooterGroup = "table-footer-group",
    TableRow = "table-row",
    TableCell = "table-cell",
    TableColumnGroup = "table-column-group",
    TableColumn = "table-column",
    TableCaption = "table-caption",
    Ruby = "ruby",
    RubyText = "ruby-text",
});

impl Display {
    /// ¿El outer display type es inline?
    pub fn is_inline_level(self) -> bool {
        matches!(
            self,
            Display::Inline
                | Display::InlineBlock
                | Display::InlineFlex
                | Display::InlineGrid
                | Display::InlineTable
                | Display::Ruby
                | Display::RubyText
        )
    }

    /// Blockification (CSS Display 3 §2.7).
    pub fn blockify(self) -> Display {
        match self {
            Display::Inline | Display::InlineBlock | Display::Ruby | Display::RubyText => {
                Display::Block
            }
            Display::InlineFlex => Display::Flex,
            Display::InlineGrid => Display::Grid,
            Display::InlineTable => Display::Table,
            Display::TableRowGroup
            | Display::TableHeaderGroup
            | Display::TableFooterGroup
            | Display::TableRow
            | Display::TableCell
            | Display::TableColumnGroup
            | Display::TableColumn
            | Display::TableCaption => Display::Block,
            other => other,
        }
    }
}

impl PropValue for Display {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let words: Vec<String> = components(input)
            .iter()
            .map(|v| ident(v).map(str::to_ascii_lowercase))
            .collect::<Option<_>>()?;
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        // Sintaxis de varias keywords (CSS Display 3 §2): <display-outside> || <display-inside>.
        Some(match words.as_slice() {
            [w] => Display::from_ident(w)?,
            ["block", "flow"] | ["flow", "block"] => Display::Block,
            ["inline", "flow"] | ["flow", "inline"] => Display::Inline,
            ["inline", "flow-root"] | ["flow-root", "inline"] => Display::InlineBlock,
            ["block", "flow-root"] | ["flow-root", "block"] => Display::FlowRoot,
            ["block", "flex"] | ["flex", "block"] => Display::Flex,
            ["inline", "flex"] | ["flex", "inline"] => Display::InlineFlex,
            ["block", "grid"] | ["grid", "block"] => Display::Grid,
            ["inline", "grid"] | ["grid", "inline"] => Display::InlineGrid,
            ["block", "table"] | ["table", "block"] => Display::Table,
            ["inline", "table"] | ["table", "inline"] => Display::InlineTable,
            ["list-item", "block"]
            | ["block", "list-item"]
            | ["list-item", "flow"]
            | ["list-item", "block", "flow"] => Display::ListItem,
            _ => return None,
        })
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

keyword_enum!(Position {
    Static = "static",
    Relative = "relative",
    Absolute = "absolute",
    Fixed = "fixed",
    Sticky = "sticky",
});
keyword_prop!(Position);

keyword_enum!(Float {
    None = "none",
    Left = "left",
    Right = "right",
});

impl PropValue for Float {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        // `inline-start`/`inline-end` se mapean con `direction: ltr` (sin writing modes aún).
        match single_ident(input)?.to_ascii_lowercase().as_str() {
            "inline-start" => Some(Float::Left),
            "inline-end" => Some(Float::Right),
            other => Float::from_ident(other),
        }
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

keyword_enum!(Clear {
    None = "none",
    Left = "left",
    Right = "right",
    Both = "both",
});

impl PropValue for Clear {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match single_ident(input)?.to_ascii_lowercase().as_str() {
            "inline-start" => Some(Clear::Left),
            "inline-end" => Some(Clear::Right),
            other => Clear::from_ident(other),
        }
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

keyword_enum!(BorderStyle {
    None = "none",
    Hidden = "hidden",
    Dotted = "dotted",
    Dashed = "dashed",
    Solid = "solid",
    Double = "double",
    Groove = "groove",
    Ridge = "ridge",
    Inset = "inset",
    Outset = "outset",
});
keyword_prop!(BorderStyle);

keyword_enum!(BoxSizing {
    ContentBox = "content-box",
    BorderBox = "border-box",
});
keyword_prop!(BoxSizing);

keyword_enum!(Overflow {
    Visible = "visible",
    Hidden = "hidden",
    Clip = "clip",
    Scroll = "scroll",
    Auto = "auto",
});
keyword_prop!(Overflow);

impl Overflow {
    /// ¿Es un scroll container? (`hidden`, `scroll` o `auto`.)
    pub fn is_scrollable(self) -> bool {
        matches!(self, Overflow::Hidden | Overflow::Scroll | Overflow::Auto)
    }
}

keyword_enum!(Visibility {
    Visible = "visible",
    Hidden = "hidden",
    Collapse = "collapse",
});
keyword_prop!(Visibility);

keyword_enum!(FontStyle {
    Normal = "normal",
    Italic = "italic",
    Oblique = "oblique",
});

impl PropValue for FontStyle {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match components(input).as_slice() {
            [v] => FontStyle::from_ident(ident(v)?),
            // `oblique <angle>`: el ángulo se acepta y se descarta por ahora.
            [o, a] if is_ident(o, "oblique") => {
                crate::values::parse_angle(a).map(|_| FontStyle::Oblique)
            }
            _ => None,
        }
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

keyword_enum!(TextAlign {
    Start = "start",
    End = "end",
    Left = "left",
    Right = "right",
    Center = "center",
    Justify = "justify",
    MatchParent = "match-parent",
});

impl PropValue for TextAlign {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        single_ident(input).and_then(TextAlign::from_ident)
    }
    /// `match-parent` computa al valor del padre con `start`/`end` resueltos según su
    /// `direction`.
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        if *self != TextAlign::MatchParent {
            return *self;
        }
        let Some(parent) = ctx.parent else {
            return TextAlign::Start;
        };
        let rtl = parent.direction == Direction::Rtl;
        match parent.text_align {
            TextAlign::Start | TextAlign::MatchParent => {
                if rtl {
                    TextAlign::Right
                } else {
                    TextAlign::Left
                }
            }
            TextAlign::End => {
                if rtl {
                    TextAlign::Left
                } else {
                    TextAlign::Right
                }
            }
            other => other,
        }
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

keyword_enum!(TextTransform {
    None = "none",
    Capitalize = "capitalize",
    Uppercase = "uppercase",
    Lowercase = "lowercase",
    FullWidth = "full-width",
});
keyword_prop!(TextTransform);

keyword_enum!(WhiteSpace {
    Normal = "normal",
    Pre = "pre",
    Nowrap = "nowrap",
    PreWrap = "pre-wrap",
    PreLine = "pre-line",
    BreakSpaces = "break-spaces",
});
keyword_prop!(WhiteSpace);

impl WhiteSpace {
    /// ¿Se colapsan los espacios y tabs? (CSS Text 3 §3, `white-space-collapse: collapse`.)
    pub fn collapses_spaces(self) -> bool {
        matches!(
            self,
            WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine
        )
    }
    /// ¿Se preservan los saltos de línea?
    pub fn preserves_newlines(self) -> bool {
        !matches!(self, WhiteSpace::Normal | WhiteSpace::Nowrap)
    }
    /// ¿Se permite el ajuste de línea?
    pub fn wraps(self) -> bool {
        !matches!(self, WhiteSpace::Pre | WhiteSpace::Nowrap)
    }
}

keyword_enum!(WordBreak {
    Normal = "normal",
    BreakAll = "break-all",
    KeepAll = "keep-all",
    BreakWord = "break-word",
});
keyword_prop!(WordBreak);

keyword_enum!(OverflowWrap {
    Normal = "normal",
    BreakWord = "break-word",
    Anywhere = "anywhere",
});
keyword_prop!(OverflowWrap);

keyword_enum!(TextDecorationStyle {
    Solid = "solid",
    Double = "double",
    Dotted = "dotted",
    Dashed = "dashed",
    Wavy = "wavy",
});
keyword_prop!(TextDecorationStyle);

keyword_enum!(ListStylePosition {
    Inside = "inside",
    Outside = "outside",
});
keyword_prop!(ListStylePosition);

keyword_enum!(Direction {
    Ltr = "ltr",
    Rtl = "rtl",
});
keyword_prop!(Direction);

keyword_enum!(UnicodeBidi {
    Normal = "normal",
    Embed = "embed",
    Isolate = "isolate",
    BidiOverride = "bidi-override",
    IsolateOverride = "isolate-override",
    Plaintext = "plaintext",
});
keyword_prop!(UnicodeBidi);

keyword_enum!(PointerEvents {
    Auto = "auto",
    None = "none",
    VisiblePainted = "visiblepainted",
    VisibleFill = "visiblefill",
    VisibleStroke = "visiblestroke",
    Visible = "visible",
    Painted = "painted",
    Fill = "fill",
    Stroke = "stroke",
    All = "all",
});
keyword_prop!(PointerEvents);

keyword_enum!(
    /// `appearance` (CSS UI 4 §7.2), con los `<compat-special>` y `<compat-auto>`.
    Appearance {
        None = "none",
        Auto = "auto",
        Base = "base",
        BaseSelect = "base-select",
        Textfield = "textfield",
        MenulistButton = "menulist-button",
        Searchfield = "searchfield",
        Textarea = "textarea",
        Checkbox = "checkbox",
        Radio = "radio",
        Menulist = "menulist",
        Listbox = "listbox",
        Meter = "meter",
        ProgressBar = "progress-bar",
        Button = "button",
        PushButton = "push-button",
        SquareButton = "square-button",
        SliderHorizontal = "slider-horizontal",
    }
);
keyword_prop!(Appearance);

keyword_enum!(CursorKeyword {
    Auto = "auto",
    Default = "default",
    None = "none",
    ContextMenu = "context-menu",
    Help = "help",
    Pointer = "pointer",
    Progress = "progress",
    Wait = "wait",
    Cell = "cell",
    Crosshair = "crosshair",
    Text = "text",
    VerticalText = "vertical-text",
    Alias = "alias",
    Copy = "copy",
    Move = "move",
    NoDrop = "no-drop",
    NotAllowed = "not-allowed",
    Grab = "grab",
    Grabbing = "grabbing",
    EResize = "e-resize",
    NResize = "n-resize",
    NeResize = "ne-resize",
    NwResize = "nw-resize",
    SResize = "s-resize",
    SeResize = "se-resize",
    SwResize = "sw-resize",
    WResize = "w-resize",
    EwResize = "ew-resize",
    NsResize = "ns-resize",
    NeswResize = "nesw-resize",
    NwseResize = "nwse-resize",
    ColResize = "col-resize",
    RowResize = "row-resize",
    AllScroll = "all-scroll",
    ZoomIn = "zoom-in",
    ZoomOut = "zoom-out",
});

/// `cursor`: `[<url> [<x> <y>]?,]* <keyword>`. Las imágenes se registran y el keyword final es
/// el fallback.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub images: Vec<String>,
    pub keyword: CursorKeyword,
}

impl PropValue for Cursor {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let groups = split_comma_separated(input);
        let (last, images) = groups.split_last()?;
        let images = images
            .iter()
            .map(|g| components(g).first().and_then(|v| parse_url(v)))
            .collect::<Option<Vec<_>>>()?;
        Some(Cursor {
            images,
            keyword: CursorKeyword::from_ident(single_ident(last)?)?,
        })
    }
    fn to_css(&self) -> String {
        let mut out = String::new();
        for url in &self.images {
            let _ = write!(out, "url({url:?}), ");
        }
        out + self.keyword.as_str()
    }
}

keyword_enum!(FlexDirection {
    Row = "row",
    RowReverse = "row-reverse",
    Column = "column",
    ColumnReverse = "column-reverse",
});
keyword_prop!(FlexDirection);

keyword_enum!(FlexWrap {
    Nowrap = "nowrap",
    Wrap = "wrap",
    WrapReverse = "wrap-reverse",
});
keyword_prop!(FlexWrap);

keyword_enum!(
    /// Valores de `justify-content` y `align-content` (CSS Box Alignment 3, sin `safe`/`unsafe`).
    ContentAlign {
        Normal = "normal",
        Start = "start",
        End = "end",
        FlexStart = "flex-start",
        FlexEnd = "flex-end",
        Center = "center",
        Left = "left",
        Right = "right",
        SpaceBetween = "space-between",
        SpaceAround = "space-around",
        SpaceEvenly = "space-evenly",
        Stretch = "stretch",
        Baseline = "baseline",
    }
);

impl PropValue for ContentAlign {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        let last = match items.as_slice() {
            [v] => *v,
            // `safe`/`unsafe <position>`, `first|last baseline`.
            [m, v] if is_ident(m, "safe") || is_ident(m, "unsafe") || is_ident(m, "first") => *v,
            _ => return None,
        };
        ContentAlign::from_ident(ident(last)?)
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

keyword_enum!(
    /// Valores de `align-items` y `align-self`.
    ItemAlign {
        Auto = "auto",
        Normal = "normal",
        Stretch = "stretch",
        Start = "start",
        End = "end",
        SelfStart = "self-start",
        SelfEnd = "self-end",
        FlexStart = "flex-start",
        FlexEnd = "flex-end",
        Center = "center",
        Baseline = "baseline",
    }
);

impl PropValue for ItemAlign {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        let last = match items.as_slice() {
            [v] => *v,
            [m, v] if is_ident(m, "safe") || is_ident(m, "unsafe") || is_ident(m, "first") => *v,
            _ => return None,
        };
        ItemAlign::from_ident(ident(last)?)
    }
    fn to_css(&self) -> String {
        self.as_str().to_owned()
    }
}

// --- Longitudes ---

impl PropValue for LengthPercentage {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => parse_length_percentage(v),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        LengthPercentage::compute(self, &ctx.len)
    }
    fn to_css(&self) -> String {
        self.to_string()
    }
}

/// `<length-percentage [0,∞]>` (padding, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct NonNegative(pub LengthPercentage);

impl PropValue for NonNegative {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => parse_non_negative_lp(v).map(NonNegative),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        NonNegative(self.0.compute(&ctx.len))
    }
    fn to_css(&self) -> String {
        self.0.to_string()
    }
}

/// `auto | <length-percentage>` (márgenes e insets).
#[derive(Debug, Clone, PartialEq)]
pub enum LengthPercentageAuto {
    Auto,
    LengthPercentage(LengthPercentage),
}

impl PropValue for LengthPercentageAuto {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "auto") => Some(LengthPercentageAuto::Auto),
            [v] => parse_length_percentage(v).map(LengthPercentageAuto::LengthPercentage),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            LengthPercentageAuto::Auto => LengthPercentageAuto::Auto,
            LengthPercentageAuto::LengthPercentage(lp) => {
                LengthPercentageAuto::LengthPercentage(lp.compute(&ctx.len))
            }
        }
    }
    fn to_css(&self) -> String {
        match self {
            LengthPercentageAuto::Auto => "auto".into(),
            LengthPercentageAuto::LengthPercentage(lp) => lp.to_string(),
        }
    }
}

/// `auto | <length-percentage [0,∞]> | min-content | max-content | fit-content`
/// (`width`, `height`, `min-*`).
#[derive(Debug, Clone, PartialEq)]
pub enum Size {
    Auto,
    LengthPercentage(LengthPercentage),
    MinContent,
    MaxContent,
    FitContent,
}

impl PropValue for Size {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => match ident(v).map(str::to_ascii_lowercase).as_deref() {
                Some("auto") => Some(Size::Auto),
                Some("min-content") => Some(Size::MinContent),
                Some("max-content") => Some(Size::MaxContent),
                Some("fit-content") => Some(Size::FitContent),
                Some(_) => None,
                None => parse_non_negative_lp(v).map(Size::LengthPercentage),
            },
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            Size::LengthPercentage(lp) => Size::LengthPercentage(lp.compute(&ctx.len)),
            other => other.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            Size::Auto => "auto".into(),
            Size::LengthPercentage(lp) => lp.to_string(),
            Size::MinContent => "min-content".into(),
            Size::MaxContent => "max-content".into(),
            Size::FitContent => "fit-content".into(),
        }
    }
}

/// `none | <length-percentage [0,∞]> | min-content | max-content | fit-content` (`max-*`).
#[derive(Debug, Clone, PartialEq)]
pub enum MaxSize {
    None,
    LengthPercentage(LengthPercentage),
    MinContent,
    MaxContent,
    FitContent,
}

impl PropValue for MaxSize {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "none") => Some(MaxSize::None),
            other => match Size::parse(other)? {
                Size::Auto => None,
                Size::LengthPercentage(lp) => Some(MaxSize::LengthPercentage(lp)),
                Size::MinContent => Some(MaxSize::MinContent),
                Size::MaxContent => Some(MaxSize::MaxContent),
                Size::FitContent => Some(MaxSize::FitContent),
            },
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            MaxSize::LengthPercentage(lp) => MaxSize::LengthPercentage(lp.compute(&ctx.len)),
            other => other.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            MaxSize::None => "none".into(),
            MaxSize::LengthPercentage(lp) => lp.to_string(),
            MaxSize::MinContent => "min-content".into(),
            MaxSize::MaxContent => "max-content".into(),
            MaxSize::FitContent => "fit-content".into(),
        }
    }
}

/// `<line-width>`: `thin | medium | thick | <length [0,∞]>`. Computado: px (el valor usado es
/// 0 si el estilo del borde es `none` o `hidden`, ver `ComputedStyle::used_border_widths`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderWidth(pub crate::values::Length);

impl BorderWidth {
    pub fn px(&self) -> f32 {
        self.0.value
    }
}

impl PropValue for BorderWidth {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let px = crate::values::Length::px;
        match trim(input) {
            [v] => match ident(v).map(str::to_ascii_lowercase).as_deref() {
                Some("thin") => Some(BorderWidth(px(1.0))),
                Some("medium") => Some(BorderWidth(px(3.0))),
                Some("thick") => Some(BorderWidth(px(5.0))),
                Some(_) => None,
                None => parse_length(v).filter(|l| l.value >= 0.0).map(BorderWidth),
            },
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BorderWidth(crate::values::Length::px(self.0.to_px(&ctx.len)))
    }
    fn to_css(&self) -> String {
        self.0.to_string()
    }
}

/// Longitud con unidad conservada hasta el cómputo (`letter-spacing`, `word-spacing`, anchos
/// de borde con unidades relativas).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spacing {
    Normal,
    Length(crate::values::Length),
}

impl PropValue for Spacing {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "normal") => Some(Spacing::Normal),
            [v] => parse_length(v).map(Spacing::Length),
            _ => None,
        }
    }
    /// `normal` computa a 0 (CSS Text 4: el valor computado es una longitud absoluta).
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        Spacing::Length(crate::values::Length::px(match self {
            Spacing::Normal => 0.0,
            Spacing::Length(l) => l.to_px(&ctx.len),
        }))
    }
    fn to_css(&self) -> String {
        match self {
            Spacing::Normal => "normal".into(),
            Spacing::Length(l) => l.to_string(),
        }
    }
}

impl Spacing {
    pub fn px(&self) -> f32 {
        match self {
            Spacing::Normal => 0.0,
            Spacing::Length(l) => l.value,
        }
    }
}

/// Un radio de esquina: `<length-percentage [0,∞]>{1,2}`.
#[derive(Debug, Clone, PartialEq)]
pub struct CornerRadius(pub LengthPercentage, pub LengthPercentage);

impl PropValue for CornerRadius {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match components(input).as_slice() {
            [h] => {
                let h = parse_non_negative_lp(h)?;
                Some(CornerRadius(h.clone(), h))
            }
            [h, v] => Some(CornerRadius(
                parse_non_negative_lp(h)?,
                parse_non_negative_lp(v)?,
            )),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        CornerRadius(self.0.compute(&ctx.len), self.1.compute(&ctx.len))
    }
    fn to_css(&self) -> String {
        if self.0 == self.1 {
            self.0.to_string()
        } else {
            format!("{} {}", self.0, self.1)
        }
    }
}

// --- Números ---

/// `auto | <integer>` (`z-index`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZIndex {
    Auto,
    Integer(i32),
}

impl PropValue for ZIndex {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "auto") => Some(ZIndex::Auto),
            [v] => integer(v).map(ZIndex::Integer),
            _ => None,
        }
    }
    fn to_css(&self) -> String {
        match self {
            ZIndex::Auto => "auto".into(),
            ZIndex::Integer(i) => i.to_string(),
        }
    }
}

/// `<integer>` (`order`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integer(pub i32);

impl PropValue for Integer {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => integer(v).map(Integer),
            _ => None,
        }
    }
    fn to_css(&self) -> String {
        self.0.to_string()
    }
}

/// `<number [0,∞]>` (`flex-grow`, `flex-shrink`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NonNegativeNumber(pub f32);

impl PropValue for NonNegativeNumber {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => parse_number(v).filter(|n| *n >= 0.0).map(NonNegativeNumber),
            _ => None,
        }
    }
    fn to_css(&self) -> String {
        fmt_number(self.0)
    }
}

/// `<opacity-value>`: número o porcentaje, computado y recortado a [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Opacity(pub f32);

impl PropValue for Opacity {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => match v.token() {
                Some(TokenKind::Percentage(p)) => Some(Opacity(p.value as f32 / 100.0)),
                _ => parse_number(v).map(Opacity),
            },
            _ => None,
        }
    }
    fn compute(&self, _ctx: &ComputeContext<'_>) -> Self {
        Opacity(self.0.clamp(0.0, 1.0))
    }
    fn to_css(&self) -> String {
        fmt_number(self.0)
    }
}

/// `flex-basis`: `auto | content | <'width'>`.
#[derive(Debug, Clone, PartialEq)]
pub enum FlexBasis {
    Content,
    Size(Size),
}

impl PropValue for FlexBasis {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "content") => Some(FlexBasis::Content),
            other => Size::parse(other).map(FlexBasis::Size),
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            FlexBasis::Content => FlexBasis::Content,
            FlexBasis::Size(s) => FlexBasis::Size(s.compute(ctx)),
        }
    }
    fn to_css(&self) -> String {
        match self {
            FlexBasis::Content => "content".into(),
            FlexBasis::Size(s) => s.to_css(),
        }
    }
}

/// `normal | <length-percentage [0,∞]>` (`row-gap`, `column-gap`).
#[derive(Debug, Clone, PartialEq)]
pub enum Gap {
    Normal,
    LengthPercentage(LengthPercentage),
}

impl PropValue for Gap {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "normal") => Some(Gap::Normal),
            [v] => parse_non_negative_lp(v).map(Gap::LengthPercentage),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            Gap::Normal => Gap::Normal,
            Gap::LengthPercentage(lp) => Gap::LengthPercentage(lp.compute(&ctx.len)),
        }
    }
    fn to_css(&self) -> String {
        match self {
            Gap::Normal => "normal".into(),
            Gap::LengthPercentage(lp) => lp.to_string(),
        }
    }
}

// --- Colores e imágenes ---

impl PropValue for Color {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => parse_color(v),
            _ => None,
        }
    }
    fn to_css(&self) -> String {
        self.to_string()
    }
}

/// Valor de la propiedad `color`: siempre un color absoluto una vez computado
/// (`currentcolor` toma el color del padre).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorProperty(pub Color);

impl ColorProperty {
    pub fn rgba(&self) -> Rgba {
        self.0.resolve(Rgba::BLACK)
    }
}

impl PropValue for ColorProperty {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        Color::parse(input).map(ColorProperty)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self.0 {
            Color::CurrentColor => ColorProperty(Color::Rgba(
                ctx.parent.map_or(Rgba::BLACK, |p| p.color.rgba()),
            )),
            c => ColorProperty(c),
        }
    }
    fn to_css(&self) -> String {
        self.0.to_string()
    }
}

keyword_enum!(BackgroundBox {
    BorderBox = "border-box",
    PaddingBox = "padding-box",
    ContentBox = "content-box",
    Text = "text",
    BorderArea = "border-area",
});

/// `background-clip`: un `<box>` por capa.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundClip(pub Vec<BackgroundBox>);

impl BackgroundClip {
    /// El valor de la capa de más abajo, que recorta el color de fondo, con `layers` capas (la
    /// lista se repite o se trunca a la cantidad de imágenes, CSS Backgrounds 3 §2.2).
    pub fn for_color(&self, layers: usize) -> BackgroundBox {
        let n = self.0.len().max(1);
        self.0
            .get((layers.max(1) - 1) % n)
            .copied()
            .unwrap_or(BackgroundBox::BorderBox)
    }
}

impl PropValue for BackgroundClip {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        split_comma_separated(input)
            .iter()
            .map(|layer| single_ident(layer).and_then(BackgroundBox::from_ident))
            .collect::<Option<Vec<_>>>()
            .filter(|v| !v.is_empty())
            .map(BackgroundClip)
    }
    fn to_css(&self) -> String {
        let v: Vec<&str> = self.0.iter().map(|b| b.as_str()).collect();
        v.join(", ")
    }
}

/// `background-image`: una `<image>` por capa (su cantidad define las capas).
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundImages(pub Vec<Image>);

impl PropValue for BackgroundImages {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        split_comma_separated(input)
            .iter()
            .map(|layer| Image::parse(layer))
            .collect::<Option<Vec<_>>>()
            .filter(|v| !v.is_empty())
            .map(BackgroundImages)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BackgroundImages(self.0.iter().map(|i| i.compute(ctx)).collect())
    }
    fn to_css(&self) -> String {
        let v: Vec<String> = self.0.iter().map(Image::to_css).collect();
        v.join(", ")
    }
}

keyword_enum!(RepeatStyle {
    Repeat = "repeat",
    Space = "space",
    Round = "round",
    NoRepeat = "no-repeat",
});

/// `background-repeat`: (horizontal, vertical) por capa.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundRepeat(pub Vec<(RepeatStyle, RepeatStyle)>);

fn parse_repeat(layer: &[ComponentValue]) -> Option<(RepeatStyle, RepeatStyle)> {
    match components(layer).as_slice() {
        [v] => match ident(v)?.to_ascii_lowercase().as_str() {
            "repeat-x" => Some((RepeatStyle::Repeat, RepeatStyle::NoRepeat)),
            "repeat-y" => Some((RepeatStyle::NoRepeat, RepeatStyle::Repeat)),
            i => RepeatStyle::from_ident(i).map(|r| (r, r)),
        },
        [a, b] => Some((
            RepeatStyle::from_ident(ident(a)?)?,
            RepeatStyle::from_ident(ident(b)?)?,
        )),
        _ => None,
    }
}

fn layer_list<T>(
    input: &[ComponentValue],
    f: impl Fn(&[ComponentValue]) -> Option<T>,
) -> Option<Vec<T>> {
    let layers = split_comma_separated(input);
    let v: Option<Vec<T>> = layers.iter().map(|l| f(l)).collect();
    v.filter(|v| !v.is_empty())
}

impl PropValue for BackgroundRepeat {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        layer_list(input, parse_repeat).map(BackgroundRepeat)
    }
    fn to_css(&self) -> String {
        let v: Vec<String> = self
            .0
            .iter()
            .map(|(x, y)| match (x, y) {
                (RepeatStyle::Repeat, RepeatStyle::NoRepeat) => "repeat-x".to_owned(),
                (RepeatStyle::NoRepeat, RepeatStyle::Repeat) => "repeat-y".to_owned(),
                (x, y) if x == y => x.as_str().to_owned(),
                (x, y) => format!("{} {}", x.as_str(), y.as_str()),
            })
            .collect();
        v.join(", ")
    }
}

/// Un eje de `background-position`: desplazamiento desde el borde inicial (left/top) o, con
/// `from_end`, desde el final (right/bottom).
#[derive(Debug, Clone, PartialEq)]
pub struct PositionAxis {
    pub from_end: bool,
    pub offset: LengthPercentage,
}

impl PositionAxis {
    /// 0% (el inicial de `background-position`).
    pub fn default_start() -> Self {
        PositionAxis::percent(0.0)
    }

    fn start(offset: LengthPercentage) -> Self {
        PositionAxis {
            from_end: false,
            offset,
        }
    }
    fn percent(p: f32) -> Self {
        PositionAxis::start(LengthPercentage::Percentage(p))
    }
    /// Posición resuelta para un espacio libre `room` (área − imagen).
    pub fn resolve(&self, room: f32) -> f32 {
        let v = self.offset.resolve(room);
        if self.from_end { room - v } else { v }
    }
}

/// `background-position`: (x, y) por capa.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundPosition(pub Vec<(PositionAxis, PositionAxis)>);

/// `<bg-position>` (CSS Backgrounds 3 §3.6): 1 a 4 valores.
pub fn parse_bg_position(layer: &[ComponentValue]) -> Option<(PositionAxis, PositionAxis)> {
    #[derive(Clone, Copy, PartialEq)]
    enum K {
        Left,
        Right,
        Top,
        Bottom,
        Center,
    }
    enum P {
        K(K),
        L(LengthPercentage),
    }
    let items: Vec<P> = components(layer)
        .iter()
        .map(|v| {
            if let Some(i) = ident(v) {
                Some(P::K(match i.to_ascii_lowercase().as_str() {
                    "left" => K::Left,
                    "right" => K::Right,
                    "top" => K::Top,
                    "bottom" => K::Bottom,
                    "center" => K::Center,
                    _ => return None,
                }))
            } else {
                parse_length_percentage(v).map(P::L)
            }
        })
        .collect::<Option<_>>()?;
    let kx = |k: K| match k {
        K::Left => Some(PositionAxis::percent(0.0)),
        K::Right => Some(PositionAxis::percent(100.0)),
        K::Center => Some(PositionAxis::percent(50.0)),
        _ => None,
    };
    let ky = |k: K| match k {
        K::Top => Some(PositionAxis::percent(0.0)),
        K::Bottom => Some(PositionAxis::percent(100.0)),
        K::Center => Some(PositionAxis::percent(50.0)),
        _ => None,
    };
    let center = || PositionAxis::percent(50.0);
    match items.as_slice() {
        [P::L(l)] => Some((PositionAxis::start(l.clone()), center())),
        [P::K(k)] => match k {
            K::Top | K::Bottom => Some((center(), ky(*k)?)),
            _ => Some((kx(*k)?, center())),
        },
        [a, b] => {
            let x = match a {
                P::L(l) => Some(PositionAxis::start(l.clone())),
                P::K(k) => kx(*k),
            };
            let y = match b {
                P::L(l) => Some(PositionAxis::start(l.clone())),
                P::K(k) => ky(*k),
            };
            if let (Some(x), Some(y)) = (x, y) {
                return Some((x, y));
            }
            // Dos keywords en orden y-x (`top left`).
            match (a, b) {
                (P::K(a), P::K(b)) => Some((kx(*b)?, ky(*a)?)),
                _ => None,
            }
        }
        _ => {
            // 3 o 4 valores: keyword [offset] keyword [offset].
            let mut x: Option<PositionAxis> = None;
            let mut y: Option<PositionAxis> = None;
            let mut i = 0;
            while i < items.len() {
                let P::K(k) = items[i] else {
                    return None;
                };
                let offset = match items.get(i + 1) {
                    Some(P::L(l)) => {
                        i += 1;
                        Some(l.clone())
                    }
                    _ => None,
                };
                i += 1;
                let axis = |end: bool| match &offset {
                    Some(o) => PositionAxis {
                        from_end: end,
                        offset: o.clone(),
                    },
                    None => PositionAxis::percent(if end { 100.0 } else { 0.0 }),
                };
                match k {
                    K::Left | K::Right if x.is_none() => x = Some(axis(k == K::Right)),
                    K::Top | K::Bottom if y.is_none() => y = Some(axis(k == K::Bottom)),
                    K::Center if offset.is_none() => {
                        if x.is_none() {
                            x = Some(center());
                        } else if y.is_none() {
                            y = Some(center());
                        } else {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            Some((x?, y?))
        }
    }
}

impl PropValue for BackgroundPosition {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        layer_list(input, parse_bg_position).map(BackgroundPosition)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BackgroundPosition(
            self.0
                .iter()
                .map(|(x, y)| {
                    let c = |a: &PositionAxis| PositionAxis {
                        from_end: a.from_end,
                        offset: a.offset.compute(&ctx.len),
                    };
                    (c(x), c(y))
                })
                .collect(),
        )
    }
    fn to_css(&self) -> String {
        let axis = |a: &PositionAxis, end: &str| {
            if a.from_end {
                format!("{end} {}", a.offset)
            } else {
                a.offset.to_string()
            }
        };
        let v: Vec<String> = self
            .0
            .iter()
            .map(|(x, y)| format!("{} {}", axis(x, "right"), axis(y, "bottom")))
            .collect();
        v.join(", ")
    }
}

/// Un valor de `background-size`.
#[derive(Debug, Clone, PartialEq)]
pub enum BgSize {
    Cover,
    Contain,
    /// Ancho y alto (`None` = `auto`).
    Explicit(Option<LengthPercentage>, Option<LengthPercentage>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundSize(pub Vec<BgSize>);

fn parse_bg_size(layer: &[ComponentValue]) -> Option<BgSize> {
    let one = |v: &ComponentValue| -> Option<Option<LengthPercentage>> {
        if is_ident(v, "auto") {
            Some(None)
        } else {
            parse_non_negative_lp(v).map(Some)
        }
    };
    match components(layer).as_slice() {
        [v] if is_ident(v, "cover") => Some(BgSize::Cover),
        [v] if is_ident(v, "contain") => Some(BgSize::Contain),
        [v] => Some(BgSize::Explicit(one(v)?, None)),
        [a, b] => Some(BgSize::Explicit(one(a)?, one(b)?)),
        _ => None,
    }
}

impl PropValue for BackgroundSize {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        layer_list(input, parse_bg_size).map(BackgroundSize)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BackgroundSize(
            self.0
                .iter()
                .map(|s| match s {
                    BgSize::Explicit(w, h) => BgSize::Explicit(
                        w.as_ref().map(|l| l.compute(&ctx.len)),
                        h.as_ref().map(|l| l.compute(&ctx.len)),
                    ),
                    s => s.clone(),
                })
                .collect(),
        )
    }
    fn to_css(&self) -> String {
        let one = |v: &Option<LengthPercentage>| {
            v.as_ref().map_or("auto".to_owned(), ToString::to_string)
        };
        let v: Vec<String> = self
            .0
            .iter()
            .map(|s| match s {
                BgSize::Cover => "cover".to_owned(),
                BgSize::Contain => "contain".to_owned(),
                BgSize::Explicit(w, h) => format!("{} {}", one(w), one(h)),
            })
            .collect();
        v.join(", ")
    }
}

/// `background-origin`: un `<visual-box>` por capa.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundOrigin(pub Vec<BackgroundBox>);

impl PropValue for BackgroundOrigin {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        layer_list(input, |l| {
            single_ident(l)
                .and_then(BackgroundBox::from_ident)
                .filter(|b| {
                    matches!(
                        b,
                        BackgroundBox::BorderBox
                            | BackgroundBox::PaddingBox
                            | BackgroundBox::ContentBox
                    )
                })
        })
        .map(BackgroundOrigin)
    }
    fn to_css(&self) -> String {
        let v: Vec<&str> = self.0.iter().map(|b| b.as_str()).collect();
        v.join(", ")
    }
}

keyword_enum!(Attachment {
    Scroll = "scroll",
    Fixed = "fixed",
    Local = "local",
});

/// `background-attachment` por capa.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundAttachment(pub Vec<Attachment>);

impl PropValue for BackgroundAttachment {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        layer_list(input, |l| single_ident(l).and_then(Attachment::from_ident))
            .map(BackgroundAttachment)
    }
    fn to_css(&self) -> String {
        let v: Vec<&str> = self.0.iter().map(|b| b.as_str()).collect();
        v.join(", ")
    }
}

/// `none | <url> | <gradient>` (CSS Images 3 §2).
#[derive(Debug, Clone, PartialEq)]
pub enum Image {
    None,
    Url(String),
    Gradient(Box<Gradient>),
}

impl PropValue for Image {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "none") => Some(Image::None),
            [v] => parse_url(v)
                .map(Image::Url)
                .or_else(|| Gradient::parse(v).map(|g| Image::Gradient(Box::new(g)))),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            Image::Gradient(g) => Image::Gradient(Box::new(g.compute(ctx))),
            other => other.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            Image::None => "none".into(),
            Image::Url(u) => format!("url({u:?})"),
            Image::Gradient(g) => g.to_css(),
        }
    }
}

// --- Fuentes ---

/// Una entrada de `font-family`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FamilyName {
    /// Familia genérica (`serif`, `sans-serif`, `monospace`, `system-ui`, ...).
    Generic(String),
    Named(String),
}

const GENERIC_FAMILIES: &[&str] = &[
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
    "ui-serif",
    "ui-sans-serif",
    "ui-monospace",
    "ui-rounded",
    "math",
    "emoji",
    "fangsong",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFamily(pub Vec<FamilyName>);

impl PropValue for FontFamily {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let mut families = Vec::new();
        for group in split_comma_separated(input) {
            let items = components(&group);
            match items.as_slice() {
                [v] if matches!(v.token(), Some(TokenKind::String(_))) => {
                    let Some(TokenKind::String(s)) = v.token() else {
                        unreachable!()
                    };
                    families.push(FamilyName::Named(s.clone()));
                }
                [v] if ident(v).is_some_and(|i| {
                    GENERIC_FAMILIES.iter().any(|g| g.eq_ignore_ascii_case(i))
                }) =>
                {
                    families.push(FamilyName::Generic(ident(v)?.to_ascii_lowercase()));
                }
                [] => return None,
                idents => {
                    // Secuencia de idents: se unen con un espacio. Los CSS-wide keywords y
                    // `default` no valen como nombre suelto.
                    let words = idents
                        .iter()
                        .map(|v| ident(v))
                        .collect::<Option<Vec<_>>>()?;
                    if words.len() == 1
                        && [
                            "initial",
                            "inherit",
                            "unset",
                            "revert",
                            "revert-layer",
                            "default",
                        ]
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(words[0]))
                    {
                        return None;
                    }
                    families.push(FamilyName::Named(words.join(" ")));
                }
            }
        }
        (!families.is_empty()).then_some(FontFamily(families))
    }
    fn to_css(&self) -> String {
        self.0
            .iter()
            .map(|f| match f {
                FamilyName::Generic(g) => g.clone(),
                FamilyName::Named(n) => format!("{n:?}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Tamaños absolutos de `font-size` (CSS Fonts 4 §2.5), como factor de `medium` (16px).
const ABSOLUTE_SIZES: &[(&str, f32)] = &[
    ("xx-small", 3.0 / 5.0),
    ("x-small", 3.0 / 4.0),
    ("small", 8.0 / 9.0),
    ("medium", 1.0),
    ("large", 6.0 / 5.0),
    ("x-large", 3.0 / 2.0),
    ("xx-large", 2.0),
    ("xxx-large", 3.0),
];

pub const MEDIUM_FONT_SIZE: f32 = 16.0;

/// `font-size`. Computado: `Px`.
#[derive(Debug, Clone, PartialEq)]
pub enum FontSize {
    Absolute(f32),
    Smaller,
    Larger,
    LengthPercentage(LengthPercentage),
    Px(f32),
}

impl FontSize {
    pub fn px(&self) -> f32 {
        match self {
            FontSize::Px(px) => *px,
            _ => MEDIUM_FONT_SIZE,
        }
    }
}

impl PropValue for FontSize {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => {
                if let Some(id) = ident(v) {
                    let lower = id.to_ascii_lowercase();
                    return match lower.as_str() {
                        "smaller" => Some(FontSize::Smaller),
                        "larger" => Some(FontSize::Larger),
                        _ => ABSOLUTE_SIZES
                            .iter()
                            .find(|(n, _)| *n == lower)
                            .map(|(_, f)| FontSize::Absolute(*f)),
                    };
                }
                parse_non_negative_lp(v).map(FontSize::LengthPercentage)
            }
            _ => None,
        }
    }
    /// `em` y los porcentajes son relativos al font-size del padre (que viene en
    /// `ctx.len.font_size` al computar `font-size`).
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        let parent = ctx.len.font_size;
        FontSize::Px(match self {
            FontSize::Absolute(f) => f * MEDIUM_FONT_SIZE,
            // Razón de 1.2 entre tamaños vecinos (CSS Fonts 4 §2.5, sugerencia).
            FontSize::Smaller => parent / 1.2,
            FontSize::Larger => parent * 1.2,
            FontSize::LengthPercentage(lp) => lp.compute(&ctx.len).resolve(parent),
            FontSize::Px(px) => *px,
        })
    }
    fn to_css(&self) -> String {
        match self {
            FontSize::Px(px) => format!("{}px", fmt_number(*px)),
            FontSize::Absolute(f) => ABSOLUTE_SIZES
                .iter()
                .find(|(_, x)| x == f)
                .map_or_else(|| "medium".into(), |(n, _)| (*n).to_owned()),
            FontSize::Smaller => "smaller".into(),
            FontSize::Larger => "larger".into(),
            FontSize::LengthPercentage(lp) => lp.to_string(),
        }
    }
}

/// `font-weight`. Computado: `Absolute`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontWeight {
    Absolute(f32),
    Bolder,
    Lighter,
}

impl FontWeight {
    pub fn value(&self) -> f32 {
        match self {
            FontWeight::Absolute(w) => *w,
            _ => 400.0,
        }
    }
}

impl PropValue for FontWeight {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => match ident(v).map(str::to_ascii_lowercase).as_deref() {
                Some("normal") => Some(FontWeight::Absolute(400.0)),
                Some("bold") => Some(FontWeight::Absolute(700.0)),
                Some("bolder") => Some(FontWeight::Bolder),
                Some("lighter") => Some(FontWeight::Lighter),
                Some(_) => None,
                None => parse_number(v)
                    .filter(|n| (1.0..=1000.0).contains(n))
                    .map(FontWeight::Absolute),
            },
            _ => None,
        }
    }
    /// Pesos relativos según la tabla de CSS Fonts 4 §2.2.
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        let parent = ctx.parent.map_or(400.0, |p| p.font_weight.value());
        FontWeight::Absolute(match self {
            FontWeight::Absolute(w) => *w,
            FontWeight::Bolder => {
                if parent < 350.0 {
                    400.0
                } else if parent < 550.0 {
                    700.0
                } else {
                    parent.max(900.0)
                }
            }
            FontWeight::Lighter => {
                if parent < 100.0 {
                    parent
                } else if parent < 550.0 {
                    100.0
                } else if parent < 750.0 {
                    400.0
                } else {
                    700.0
                }
            }
        })
    }
    fn to_css(&self) -> String {
        match self {
            FontWeight::Absolute(w) => fmt_number(*w),
            FontWeight::Bolder => "bolder".into(),
            FontWeight::Lighter => "lighter".into(),
        }
    }
}

/// `line-height`. Computado: `normal`, un número, o una longitud absoluta.
#[derive(Debug, Clone, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    LengthPercentage(LengthPercentage),
}

impl LineHeight {
    /// Valor usado en px para un font-size dado; `normal` usa `normal_factor` (de las métricas).
    pub fn resolve(&self, font_size: f32, normal_factor: f32) -> f32 {
        match self {
            LineHeight::Normal => font_size * normal_factor,
            LineHeight::Number(n) => font_size * n,
            LineHeight::LengthPercentage(lp) => lp.resolve(font_size),
        }
    }
}

impl PropValue for LineHeight {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] if is_ident(v, "normal") => Some(LineHeight::Normal),
            [v] => match parse_number(v) {
                Some(n) if n >= 0.0 => Some(LineHeight::Number(n)),
                Some(_) => None,
                None => parse_non_negative_lp(v).map(LineHeight::LengthPercentage),
            },
            _ => None,
        }
    }
    /// Los porcentajes se resuelven contra el font-size del elemento.
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            LineHeight::LengthPercentage(lp) => LineHeight::LengthPercentage(LengthPercentage::px(
                lp.compute(&ctx.len).resolve(ctx.len.font_size),
            )),
            other => other.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            LineHeight::Normal => "normal".into(),
            LineHeight::Number(n) => fmt_number(*n),
            LineHeight::LengthPercentage(lp) => lp.to_string(),
        }
    }
}

// --- Texto ---

keyword_enum!(VerticalAlignKeyword {
    Baseline = "baseline",
    Sub = "sub",
    Super = "super",
    Top = "top",
    TextTop = "text-top",
    Middle = "middle",
    Bottom = "bottom",
    TextBottom = "text-bottom",
});

#[derive(Debug, Clone, PartialEq)]
pub enum VerticalAlign {
    Keyword(VerticalAlignKeyword),
    /// Desplazamiento de la baseline; los porcentajes son relativos al `line-height`.
    LengthPercentage(LengthPercentage),
}

impl PropValue for VerticalAlign {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => match ident(v) {
                Some(i) => VerticalAlignKeyword::from_ident(i).map(VerticalAlign::Keyword),
                None => parse_length_percentage(v).map(VerticalAlign::LengthPercentage),
            },
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        match self {
            VerticalAlign::LengthPercentage(lp) => {
                VerticalAlign::LengthPercentage(lp.compute(&ctx.len))
            }
            other => other.clone(),
        }
    }
    fn to_css(&self) -> String {
        match self {
            VerticalAlign::Keyword(k) => k.as_str().into(),
            VerticalAlign::LengthPercentage(lp) => lp.to_string(),
        }
    }
}

/// `text-decoration-line`: `none | [ underline || overline || line-through || blink ]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextDecorationLine {
    pub underline: bool,
    pub overline: bool,
    pub line_through: bool,
    pub blink: bool,
}

impl PropValue for TextDecorationLine {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        let mut line = TextDecorationLine::default();
        if let [v] = items.as_slice()
            && is_ident(v, "none")
        {
            return Some(line);
        }
        if items.is_empty() {
            return None;
        }
        for v in items {
            let flag = match ident(v)?.to_ascii_lowercase().as_str() {
                "underline" => &mut line.underline,
                "overline" => &mut line.overline,
                "line-through" => &mut line.line_through,
                "blink" => &mut line.blink,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
        Some(line)
    }
    fn to_css(&self) -> String {
        let mut parts = Vec::new();
        if self.underline {
            parts.push("underline");
        }
        if self.overline {
            parts.push("overline");
        }
        if self.line_through {
            parts.push("line-through");
        }
        if self.blink {
            parts.push("blink");
        }
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(" ")
        }
    }
}

/// `list-style-type`: `<counter-style-name> | <string> | none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListStyleType {
    None,
    /// Nombre de counter style en minúsculas (`disc`, `decimal`, `lower-roman`, ...).
    CounterStyle(String),
    String(String),
}

impl PropValue for ListStyleType {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => match v.token()? {
                TokenKind::Ident(i) if i.eq_ignore_ascii_case("none") => Some(ListStyleType::None),
                TokenKind::Ident(i) => Some(ListStyleType::CounterStyle(i.to_ascii_lowercase())),
                TokenKind::String(s) => Some(ListStyleType::String(s.clone())),
                _ => None,
            },
            _ => None,
        }
    }
    fn to_css(&self) -> String {
        match self {
            ListStyleType::None => "none".into(),
            ListStyleType::CounterStyle(n) => n.clone(),
            ListStyleType::String(s) => format!("{s:?}"),
        }
    }
}

/// Un elemento de `content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentItem {
    String(String),
    Url(String),
    /// `attr(name)`: se resuelve contra el elemento al generar las cajas.
    Attr(String),
    /// `counter(name[, style])` / `counters(...)`: solo se parsea por ahora.
    Counter(String, Option<String>),
    OpenQuote,
    CloseQuote,
    NoOpenQuote,
    NoCloseQuote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Normal,
    None,
    Items(Vec<ContentItem>),
}

impl PropValue for Content {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        if let [v] = items.as_slice() {
            if is_ident(v, "normal") {
                return Some(Content::Normal);
            }
            if is_ident(v, "none") {
                return Some(Content::None);
            }
        }
        // `<content-list> [ / <string>+ ]?`: el texto alternativo se descarta.
        let end = items
            .iter()
            .position(|v| v.token() == Some(&TokenKind::Delim('/')))
            .unwrap_or(items.len());
        let mut out = Vec::new();
        for v in &items[..end] {
            let item = match v {
                ComponentValue::Token(t) => match &t.kind {
                    TokenKind::String(s) => ContentItem::String(s.clone()),
                    TokenKind::Url(u) => ContentItem::Url(u.clone()),
                    TokenKind::Ident(i) => match i.to_ascii_lowercase().as_str() {
                        "open-quote" => ContentItem::OpenQuote,
                        "close-quote" => ContentItem::CloseQuote,
                        "no-open-quote" => ContentItem::NoOpenQuote,
                        "no-close-quote" => ContentItem::NoCloseQuote,
                        _ => return None,
                    },
                    _ => return None,
                },
                ComponentValue::Function(f) => match f.name.to_ascii_lowercase().as_str() {
                    "attr" => ContentItem::Attr(ident(components(&f.value).first()?)?.to_owned()),
                    "counter" | "counters" => {
                        let args = split_comma_separated(&f.value);
                        let name = single_ident(args.first()?)?.to_owned();
                        let style = args
                            .last()
                            .filter(|_| args.len() > 1)
                            .and_then(|a| single_ident(a))
                            .map(str::to_owned);
                        ContentItem::Counter(name, style)
                    }
                    "url" => ContentItem::Url(parse_url(v)?),
                    _ => return None,
                },
                ComponentValue::Block(_) => return None,
            };
            out.push(item);
        }
        (!out.is_empty()).then_some(Content::Items(out))
    }
    fn to_css(&self) -> String {
        match self {
            Content::Normal => "normal".into(),
            Content::None => "none".into(),
            Content::Items(items) => items
                .iter()
                .map(|i| match i {
                    ContentItem::String(s) => format!("{s:?}"),
                    ContentItem::Url(u) => format!("url({u:?})"),
                    ContentItem::Attr(a) => format!("attr({a})"),
                    ContentItem::Counter(n, s) => match s {
                        Some(s) => format!("counter({n}, {s})"),
                        None => format!("counter({n})"),
                    },
                    ContentItem::OpenQuote => "open-quote".into(),
                    ContentItem::CloseQuote => "close-quote".into(),
                    ContentItem::NoOpenQuote => "no-open-quote".into(),
                    ContentItem::NoCloseQuote => "no-close-quote".into(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// `quotes`: `auto | none | [ <string> <string> ]+`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quotes {
    Auto,
    None,
    Pairs(Vec<(String, String)>),
}

impl PropValue for Quotes {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let items = components(input);
        if let [v] = items.as_slice() {
            if is_ident(v, "auto") {
                return Some(Quotes::Auto);
            }
            if is_ident(v, "none") {
                return Some(Quotes::None);
            }
        }
        if items.is_empty() || !items.len().is_multiple_of(2) {
            return None;
        }
        let strings = items
            .iter()
            .map(|v| match v.token() {
                Some(TokenKind::String(s)) => Some(s.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Quotes::Pairs(
            strings
                .chunks(2)
                .map(|p| (p[0].clone(), p[1].clone()))
                .collect(),
        ))
    }
    fn to_css(&self) -> String {
        match self {
            Quotes::Auto => "auto".into(),
            Quotes::None => "none".into(),
            Quotes::Pairs(p) => p
                .iter()
                .map(|(a, b)| format!("{a:?} {b:?}"))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

// --- Tablas (CSS 2.2 §17) ---

keyword_enum!(
    /// `border-collapse` (CSS 2.2 §17.6).
    BorderCollapse {
        Separate = "separate",
        Collapse = "collapse",
    }
);
keyword_prop!(BorderCollapse);

keyword_enum!(
    /// `caption-side` (CSS 2.2 §17.4.1).
    CaptionSide {
        Top = "top",
        Bottom = "bottom",
    }
);
keyword_prop!(CaptionSide);

keyword_enum!(
    /// `table-layout` (CSS 2.2 §17.5.2).
    TableLayout {
        Auto = "auto",
        Fixed = "fixed",
    }
);
keyword_prop!(TableLayout);

keyword_enum!(
    /// `empty-cells` (CSS 2.2 §17.6.1.1).
    EmptyCells {
        Show = "show",
        Hide = "hide",
    }
);
keyword_prop!(EmptyCells);

/// Una `<length>` (o `calc()` sin porcentajes).
pub(crate) fn parse_length_px(v: &ComponentValue) -> Option<LengthPercentage> {
    parse_length_percentage(v).filter(|lp| !lp.has_percentage())
}

fn is_negative_length(lp: &LengthPercentage) -> bool {
    matches!(lp, LengthPercentage::Length(l) if l.value < 0.0)
}

/// `border-spacing: <length [0,∞]> <length [0,∞]>?` (CSS 2.2 §17.6.1): horizontal y vertical.
#[derive(Debug, Clone, PartialEq)]
pub struct BorderSpacing {
    pub horizontal: LengthPercentage,
    pub vertical: LengthPercentage,
}

impl BorderSpacing {
    /// (horizontal, vertical) en px.
    pub fn px(&self) -> (f32, f32) {
        (self.horizontal.resolve(0.0), self.vertical.resolve(0.0))
    }
}

impl PropValue for BorderSpacing {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let non_negative =
            |v: &ComponentValue| parse_length_px(v).filter(|l| !is_negative_length(l));
        match components(input).as_slice() {
            [a] => {
                let a = non_negative(a)?;
                Some(BorderSpacing {
                    horizontal: a.clone(),
                    vertical: a,
                })
            }
            [a, b] => Some(BorderSpacing {
                horizontal: non_negative(a)?,
                vertical: non_negative(b)?,
            }),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BorderSpacing {
            horizontal: self.horizontal.compute(&ctx.len),
            vertical: self.vertical.compute(&ctx.len),
        }
    }
    fn to_css(&self) -> String {
        if self.horizontal == self.vertical {
            self.horizontal.to_string()
        } else {
            format!("{} {}", self.horizontal, self.vertical)
        }
    }
}

// --- Outline (CSS UI 4 §5) ---

/// `outline-style`: `auto | <outline-line-style>` (los estilos de borde salvo `hidden`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineStyle {
    Auto,
    Style(BorderStyle),
}

impl OutlineStyle {
    /// El estilo con que se pinta: `auto` como `solid` (dependiente del UA).
    pub fn border_style(self) -> BorderStyle {
        match self {
            OutlineStyle::Auto => BorderStyle::Solid,
            OutlineStyle::Style(s) => s,
        }
    }

    /// ¿Se pinta? (`none` no).
    pub fn is_visible(self) -> bool {
        self != OutlineStyle::Style(BorderStyle::None)
    }
}

impl PropValue for OutlineStyle {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        let name = single_ident(input)?;
        if name.eq_ignore_ascii_case("auto") {
            return Some(OutlineStyle::Auto);
        }
        match BorderStyle::from_ident(name)? {
            BorderStyle::Hidden => None,
            s => Some(OutlineStyle::Style(s)),
        }
    }
    fn to_css(&self) -> String {
        match self {
            OutlineStyle::Auto => "auto".into(),
            OutlineStyle::Style(s) => s.as_str().into(),
        }
    }
}

/// `outline-color`: `auto | <color>`; `auto` (y el legado `invert`) se pinta como
/// `currentcolor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineColor(pub Color);

impl PropValue for OutlineColor {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        if let Some(name) = single_ident(input)
            && (name.eq_ignore_ascii_case("auto") || name.eq_ignore_ascii_case("invert"))
        {
            return Some(OutlineColor(Color::CurrentColor));
        }
        Color::parse(input).map(OutlineColor)
    }
    fn to_css(&self) -> String {
        self.0.to_string()
    }
}

/// `<length>` con signo computada a px (`outline-offset`).
#[derive(Debug, Clone, PartialEq)]
pub struct LengthPx(pub LengthPercentage);

impl LengthPx {
    pub fn px(&self) -> f32 {
        self.0.resolve(0.0)
    }
}

impl PropValue for LengthPx {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        match trim(input) {
            [v] => parse_length_px(v).map(LengthPx),
            _ => None,
        }
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        LengthPx(self.0.compute(&ctx.len))
    }
    fn to_css(&self) -> String {
        self.0.to_string()
    }
}

// --- Sombras (CSS Backgrounds 3 §7.1, CSS Text Decoration 3 §4) ---

/// Una sombra. Computada, las longitudes están en px.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub color: Color,
    pub x: LengthPercentage,
    pub y: LengthPercentage,
    pub blur: LengthPercentage,
    pub spread: LengthPercentage,
    pub inset: bool,
}

impl Shadow {
    /// (x, y, blur, spread) en px.
    pub fn px(&self) -> (f32, f32, f32, f32) {
        (
            self.x.resolve(0.0),
            self.y.resolve(0.0),
            self.blur.resolve(0.0).max(0.0),
            self.spread.resolve(0.0),
        )
    }

    /// `<shadow>`: `<color>? && [<length>{2} <length [0,∞]>? <length>?] && inset?`. Solo
    /// `box-shadow` admite `inset` y la extensión.
    fn parse(input: &[ComponentValue], box_shadow: bool) -> Option<Shadow> {
        let items = components(input);
        let mut color = None;
        let mut inset = false;
        let mut lengths: Vec<LengthPercentage> = Vec::new();
        // Las longitudes van juntas: una vez cerrado el grupo no puede haber otra.
        let mut lengths_closed = false;
        for v in items {
            if let Some(l) = parse_length_px(v) {
                if lengths_closed {
                    return None;
                }
                lengths.push(l);
                continue;
            }
            if !lengths.is_empty() {
                lengths_closed = true;
            }
            if box_shadow && !inset && is_ident(v, "inset") {
                inset = true;
                continue;
            }
            if color.is_none()
                && let Some(c) = parse_color(v)
            {
                color = Some(c);
                continue;
            }
            return None;
        }
        let max = if box_shadow { 4 } else { 3 };
        if lengths.len() < 2 || lengths.len() > max {
            return None;
        }
        // El radio de desenfoque no puede ser negativo.
        if lengths.get(2).is_some_and(is_negative_length) {
            return None;
        }
        let zero = LengthPercentage::zero;
        Some(Shadow {
            color: color.unwrap_or(Color::CurrentColor),
            x: lengths[0].clone(),
            y: lengths[1].clone(),
            blur: lengths.get(2).cloned().unwrap_or_else(zero),
            spread: lengths.get(3).cloned().unwrap_or_else(zero),
            inset,
        })
    }

    fn compute(&self, ctx: &ComputeContext<'_>) -> Shadow {
        Shadow {
            color: self.color,
            x: self.x.compute(&ctx.len),
            y: self.y.compute(&ctx.len),
            blur: self.blur.compute(&ctx.len),
            spread: self.spread.compute(&ctx.len),
            inset: self.inset,
        }
    }

    fn to_css(&self, box_shadow: bool) -> String {
        let mut s = String::new();
        if self.inset {
            s.push_str("inset ");
        }
        let _ = write!(s, "{} {} {} {}", self.color, self.x, self.y, self.blur);
        if box_shadow {
            let _ = write!(s, " {}", self.spread);
        }
        s
    }
}

fn parse_shadows(input: &[ComponentValue], box_shadow: bool) -> Option<Vec<Shadow>> {
    if single_ident(input).is_some_and(|i| i.eq_ignore_ascii_case("none")) {
        return Some(Vec::new());
    }
    split_comma_separated(input)
        .iter()
        .map(|s| Shadow::parse(s, box_shadow))
        .collect()
}

fn shadows_css(v: &[Shadow], box_shadow: bool) -> String {
    if v.is_empty() {
        return "none".into();
    }
    v.iter()
        .map(|s| s.to_css(box_shadow))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `box-shadow: none | <shadow>#` (la primera de la lista queda arriba).
#[derive(Debug, Clone, PartialEq)]
pub struct BoxShadow(pub Vec<Shadow>);

impl PropValue for BoxShadow {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        parse_shadows(input, true).map(BoxShadow)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        BoxShadow(self.0.iter().map(|s| s.compute(ctx)).collect())
    }
    fn to_css(&self) -> String {
        shadows_css(&self.0, true)
    }
}

/// `text-shadow: none | <shadow>#` (sin `inset` ni extensión).
#[derive(Debug, Clone, PartialEq)]
pub struct TextShadow(pub Vec<Shadow>);

impl PropValue for TextShadow {
    fn parse(input: &[ComponentValue]) -> Option<Self> {
        parse_shadows(input, false).map(TextShadow)
    }
    fn compute(&self, ctx: &ComputeContext<'_>) -> Self {
        TextShadow(self.0.iter().map(|s| s.compute(ctx)).collect())
    }
    fn to_css(&self) -> String {
        shadows_css(&self.0, false)
    }
}

pub use super::gradient::*; // E4
pub use super::grid::*; // E4
