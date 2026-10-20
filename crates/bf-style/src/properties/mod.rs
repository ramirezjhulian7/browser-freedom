//! Registry de propiedades: una tabla única ([`PROPERTIES`]) con nombre, herencia, valor
//! inicial, parseo, cómputo y acceso al campo tipado de [`ComputedStyle`] de cada longhand.
//! Los shorthands están en [`shorthands`].

pub mod gradient; // E4
pub mod grid; // E4
pub mod shorthands;
pub mod types;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use bf_css::syntax::ComponentValue;

use crate::values::{Color, LengthContext, LengthPercentage};
use types::*;

/// Contexto de cómputo de un elemento.
#[derive(Debug, Clone, Copy)]
pub struct ComputeContext<'a> {
    /// Longitudes relativas: font-size y line-height del elemento (para `font-size` mismo, los
    /// del padre).
    pub len: LengthContext,
    pub parent: Option<&'a ComputedStyle>,
}

/// Tipo de valor de una propiedad: parseo, cómputo y serialización.
pub trait PropValue: Clone + fmt::Debug + PartialEq + Send + Sync + 'static {
    /// Parsea el valor (ya sin whitespace en los extremos). `None` si es inválido.
    fn parse(input: &[ComponentValue]) -> Option<Self>;
    /// Valor computado a partir del especificado.
    fn compute(&self, _ctx: &ComputeContext<'_>) -> Self {
        self.clone()
    }
    /// Serialización aproximada (para volcados y tests).
    fn to_css(&self) -> String;
}

/// Custom properties computadas (`--*`): nombre → component values. Se heredan.
pub type CustomProperties = HashMap<String, Arc<Vec<ComponentValue>>>;

/// Definición de un longhand en el registry.
pub struct PropertyDef {
    pub id: LonghandId,
    pub name: &'static str,
    pub inherited: bool,
    /// Valor inicial como texto CSS (se parsea y computa una vez).
    pub initial: &'static str,
    pub animatable: bool,
    pub parse: fn(&[ComponentValue]) -> Option<Value>,
    pub compute: fn(&Value, &ComputeContext<'_>) -> Value,
    pub get: fn(&ComputedStyle) -> Value,
    pub set: fn(&mut ComputedStyle, Value),
    pub to_css: fn(&Value) -> String,
}

macro_rules! longhands {
    ($( $id:ident $name:literal $field:ident : $ty:ty = $initial:literal, inherited: $inh:literal, animatable: $anim:literal; )+) => {
        /// Identificador de un longhand (índice en [`PROPERTIES`]).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum LonghandId { $($id),+ }

        /// Valor de un longhand (especificado o computado).
        #[derive(Debug, Clone, PartialEq)]
        pub enum Value { $($id($ty)),+ }

        /// Estilo computado de un elemento o pseudo-elemento.
        #[derive(Debug, Clone, PartialEq)]
        pub struct ComputedStyle {
            $(pub $field: $ty,)+
            pub custom: Arc<CustomProperties>,
            /// `display` antes de la blockificación (para la posición estática de las cajas
            /// absolutas, CSS Position 3 §4.3).
            pub original_display: types::Display,
        }

        /// La tabla de longhands, indexada por `LonghandId as usize`.
        pub static PROPERTIES: &[PropertyDef] = &[
            $(PropertyDef {
                id: LonghandId::$id,
                name: $name,
                inherited: $inh,
                initial: $initial,
                animatable: $anim,
                parse: |input| <$ty as PropValue>::parse(input).map(Value::$id),
                compute: |value, ctx| match value {
                    Value::$id(v) => Value::$id(<$ty as PropValue>::compute(v, ctx)),
                    other => other.clone(),
                },
                get: |style| Value::$id(style.$field.clone()),
                set: |style, value| {
                    if let Value::$id(v) = value {
                        style.$field = v;
                    }
                },
                to_css: |value| match value {
                    Value::$id(v) => v.to_css(),
                    _ => String::new(),
                },
            },)+
        ];

        impl Value {
            pub fn id(&self) -> LonghandId {
                match self { $(Value::$id(_) => LonghandId::$id),+ }
            }
        }

        fn build_initial_style(values: &[Value]) -> ComputedStyle {
            let mut i = 0..;
            let mut next = || values[i.next().expect("índice")].clone();
            ComputedStyle {
                $($field: match next() { Value::$id(v) => v, _ => unreachable!() },)+
                custom: Arc::new(CustomProperties::new()),
                original_display: types::Display::Inline,
            }
        }
    };
}

longhands! {
    Display "display" display: Display = "inline", inherited: false, animatable: false;
    Position "position" position: Position = "static", inherited: false, animatable: false;
    Top "top" top: LengthPercentageAuto = "auto", inherited: false, animatable: true;
    Right "right" right: LengthPercentageAuto = "auto", inherited: false, animatable: true;
    Bottom "bottom" bottom: LengthPercentageAuto = "auto", inherited: false, animatable: true;
    Left "left" left: LengthPercentageAuto = "auto", inherited: false, animatable: true;
    Float "float" float: Float = "none", inherited: false, animatable: false;
    Clear "clear" clear: Clear = "none", inherited: false, animatable: false;
    ZIndex "z-index" z_index: ZIndex = "auto", inherited: false, animatable: true;
    Width "width" width: Size = "auto", inherited: false, animatable: true;
    Height "height" height: Size = "auto", inherited: false, animatable: true;
    MinWidth "min-width" min_width: Size = "auto", inherited: false, animatable: true;
    MinHeight "min-height" min_height: Size = "auto", inherited: false, animatable: true;
    MaxWidth "max-width" max_width: MaxSize = "none", inherited: false, animatable: true;
    MaxHeight "max-height" max_height: MaxSize = "none", inherited: false, animatable: true;
    MarginTop "margin-top" margin_top: LengthPercentageAuto = "0", inherited: false, animatable: true;
    MarginRight "margin-right" margin_right: LengthPercentageAuto = "0", inherited: false, animatable: true;
    MarginBottom "margin-bottom" margin_bottom: LengthPercentageAuto = "0", inherited: false, animatable: true;
    MarginLeft "margin-left" margin_left: LengthPercentageAuto = "0", inherited: false, animatable: true;
    PaddingTop "padding-top" padding_top: NonNegative = "0", inherited: false, animatable: true;
    PaddingRight "padding-right" padding_right: NonNegative = "0", inherited: false, animatable: true;
    PaddingBottom "padding-bottom" padding_bottom: NonNegative = "0", inherited: false, animatable: true;
    PaddingLeft "padding-left" padding_left: NonNegative = "0", inherited: false, animatable: true;
    BorderTopWidth "border-top-width" border_top_width: BorderWidth = "medium", inherited: false, animatable: true;
    BorderRightWidth "border-right-width" border_right_width: BorderWidth = "medium", inherited: false, animatable: true;
    BorderBottomWidth "border-bottom-width" border_bottom_width: BorderWidth = "medium", inherited: false, animatable: true;
    BorderLeftWidth "border-left-width" border_left_width: BorderWidth = "medium", inherited: false, animatable: true;
    BorderTopStyle "border-top-style" border_top_style: BorderStyle = "none", inherited: false, animatable: false;
    BorderRightStyle "border-right-style" border_right_style: BorderStyle = "none", inherited: false, animatable: false;
    BorderBottomStyle "border-bottom-style" border_bottom_style: BorderStyle = "none", inherited: false, animatable: false;
    BorderLeftStyle "border-left-style" border_left_style: BorderStyle = "none", inherited: false, animatable: false;
    BorderTopColor "border-top-color" border_top_color: Color = "currentcolor", inherited: false, animatable: true;
    BorderRightColor "border-right-color" border_right_color: Color = "currentcolor", inherited: false, animatable: true;
    BorderBottomColor "border-bottom-color" border_bottom_color: Color = "currentcolor", inherited: false, animatable: true;
    BorderLeftColor "border-left-color" border_left_color: Color = "currentcolor", inherited: false, animatable: true;
    BorderTopLeftRadius "border-top-left-radius" border_top_left_radius: CornerRadius = "0", inherited: false, animatable: true;
    BorderTopRightRadius "border-top-right-radius" border_top_right_radius: CornerRadius = "0", inherited: false, animatable: true;
    BorderBottomRightRadius "border-bottom-right-radius" border_bottom_right_radius: CornerRadius = "0", inherited: false, animatable: true;
    BorderBottomLeftRadius "border-bottom-left-radius" border_bottom_left_radius: CornerRadius = "0", inherited: false, animatable: true;
    BoxSizing "box-sizing" box_sizing: BoxSizing = "content-box", inherited: false, animatable: false;
    OverflowX "overflow-x" overflow_x: Overflow = "visible", inherited: false, animatable: false;
    OverflowY "overflow-y" overflow_y: Overflow = "visible", inherited: false, animatable: false;
    Visibility "visibility" visibility: Visibility = "visible", inherited: true, animatable: true;
    Opacity "opacity" opacity: Opacity = "1", inherited: false, animatable: true;
    Color "color" color: ColorProperty = "canvastext", inherited: true, animatable: true;
    BackgroundColor "background-color" background_color: Color = "transparent", inherited: false, animatable: true;
    BackgroundImage "background-image" background_image: BackgroundImages = "none", inherited: false, animatable: false;
    BackgroundClip "background-clip" background_clip: BackgroundClip = "border-box", inherited: false, animatable: false;
    BackgroundRepeat "background-repeat" background_repeat: BackgroundRepeat = "repeat", inherited: false, animatable: false;
    BackgroundPosition "background-position" background_position: BackgroundPosition = "0% 0%", inherited: false, animatable: true;
    BackgroundSize "background-size" background_size: BackgroundSize = "auto", inherited: false, animatable: true;
    BackgroundOrigin "background-origin" background_origin: BackgroundOrigin = "padding-box", inherited: false, animatable: false;
    BackgroundAttachment "background-attachment" background_attachment: BackgroundAttachment = "scroll", inherited: false, animatable: false;
    FontFamily "font-family" font_family: FontFamily = "serif", inherited: true, animatable: false;
    FontSize "font-size" font_size: FontSize = "medium", inherited: true, animatable: true;
    FontWeight "font-weight" font_weight: FontWeight = "normal", inherited: true, animatable: true;
    FontStyle "font-style" font_style: FontStyle = "normal", inherited: true, animatable: false;
    LineHeight "line-height" line_height: LineHeight = "normal", inherited: true, animatable: true;
    TextAlign "text-align" text_align: TextAlign = "start", inherited: true, animatable: false;
    TextDecorationLine "text-decoration-line" text_decoration_line: TextDecorationLine = "none", inherited: false, animatable: false;
    TextDecorationColor "text-decoration-color" text_decoration_color: Color = "currentcolor", inherited: false, animatable: true;
    TextDecorationStyle "text-decoration-style" text_decoration_style: TextDecorationStyle = "solid", inherited: false, animatable: false;
    TextTransform "text-transform" text_transform: TextTransform = "none", inherited: true, animatable: false;
    WhiteSpace "white-space" white_space: WhiteSpace = "normal", inherited: true, animatable: false;
    WordBreak "word-break" word_break: WordBreak = "normal", inherited: true, animatable: false;
    OverflowWrap "overflow-wrap" overflow_wrap: OverflowWrap = "normal", inherited: true, animatable: false;
    LetterSpacing "letter-spacing" letter_spacing: Spacing = "normal", inherited: true, animatable: true;
    WordSpacing "word-spacing" word_spacing: Spacing = "normal", inherited: true, animatable: true;
    VerticalAlign "vertical-align" vertical_align: VerticalAlign = "baseline", inherited: false, animatable: true;
    TextIndent "text-indent" text_indent: LengthPercentage = "0", inherited: true, animatable: true;
    ListStyleType "list-style-type" list_style_type: ListStyleType = "disc", inherited: true, animatable: false;
    ListStylePosition "list-style-position" list_style_position: ListStylePosition = "outside", inherited: true, animatable: false;
    ListStyleImage "list-style-image" list_style_image: Image = "none", inherited: true, animatable: false;
    Content "content" content: Content = "normal", inherited: false, animatable: false;
    Quotes "quotes" quotes: Quotes = "auto", inherited: true, animatable: false;
    Cursor "cursor" cursor: Cursor = "auto", inherited: true, animatable: false;
    PointerEvents "pointer-events" pointer_events: PointerEvents = "auto", inherited: true, animatable: false;
    Appearance "appearance" appearance: Appearance = "none", inherited: false, animatable: false;
    Direction "direction" direction: Direction = "ltr", inherited: true, animatable: false;
    UnicodeBidi "unicode-bidi" unicode_bidi: UnicodeBidi = "normal", inherited: false, animatable: false;
    FlexDirection "flex-direction" flex_direction: FlexDirection = "row", inherited: false, animatable: false;
    FlexWrap "flex-wrap" flex_wrap: FlexWrap = "nowrap", inherited: false, animatable: false;
    FlexGrow "flex-grow" flex_grow: NonNegativeNumber = "0", inherited: false, animatable: true;
    FlexShrink "flex-shrink" flex_shrink: NonNegativeNumber = "1", inherited: false, animatable: true;
    FlexBasis "flex-basis" flex_basis: FlexBasis = "auto", inherited: false, animatable: true;
    JustifyContent "justify-content" justify_content: ContentAlign = "normal", inherited: false, animatable: false;
    AlignContent "align-content" align_content: ContentAlign = "normal", inherited: false, animatable: false;
    AlignItems "align-items" align_items: ItemAlign = "normal", inherited: false, animatable: false;
    AlignSelf "align-self" align_self: ItemAlign = "auto", inherited: false, animatable: false;
    JustifyItems "justify-items" justify_items: ItemAlign = "normal", inherited: false, animatable: false;
    JustifySelf "justify-self" justify_self: ItemAlign = "auto", inherited: false, animatable: false;
    Order "order" order: Integer = "0", inherited: false, animatable: true;
    RowGap "row-gap" row_gap: Gap = "normal", inherited: false, animatable: true;
    ColumnGap "column-gap" column_gap: Gap = "normal", inherited: false, animatable: true;
    // E4: tablas, outline, sombras y grid.
    BorderCollapse "border-collapse" border_collapse: BorderCollapse = "separate", inherited: true, animatable: false;
    BorderSpacing "border-spacing" border_spacing: BorderSpacing = "0", inherited: true, animatable: true;
    CaptionSide "caption-side" caption_side: CaptionSide = "top", inherited: true, animatable: false;
    TableLayout "table-layout" table_layout: TableLayout = "auto", inherited: false, animatable: false;
    EmptyCells "empty-cells" empty_cells: EmptyCells = "show", inherited: true, animatable: false;
    OutlineWidth "outline-width" outline_width: BorderWidth = "medium", inherited: false, animatable: true;
    OutlineStyle "outline-style" outline_style: OutlineStyle = "none", inherited: false, animatable: false;
    OutlineColor "outline-color" outline_color: OutlineColor = "auto", inherited: false, animatable: true;
    OutlineOffset "outline-offset" outline_offset: LengthPx = "0", inherited: false, animatable: true;
    BoxShadow "box-shadow" box_shadow: BoxShadow = "none", inherited: false, animatable: true;
    TextShadow "text-shadow" text_shadow: TextShadow = "none", inherited: true, animatable: true;
    GridTemplateColumns "grid-template-columns" grid_template_columns: GridTemplate = "none", inherited: false, animatable: true;
    GridTemplateRows "grid-template-rows" grid_template_rows: GridTemplate = "none", inherited: false, animatable: true;
    GridTemplateAreas "grid-template-areas" grid_template_areas: GridTemplateAreas = "none", inherited: false, animatable: false;
    GridAutoColumns "grid-auto-columns" grid_auto_columns: GridAutoTracks = "auto", inherited: false, animatable: true;
    GridAutoRows "grid-auto-rows" grid_auto_rows: GridAutoTracks = "auto", inherited: false, animatable: true;
    GridAutoFlow "grid-auto-flow" grid_auto_flow: GridAutoFlow = "row", inherited: false, animatable: false;
    GridRowStart "grid-row-start" grid_row_start: GridLine = "auto", inherited: false, animatable: false;
    GridRowEnd "grid-row-end" grid_row_end: GridLine = "auto", inherited: false, animatable: false;
    GridColumnStart "grid-column-start" grid_column_start: GridLine = "auto", inherited: false, animatable: false;
    GridColumnEnd "grid-column-end" grid_column_end: GridLine = "auto", inherited: false, animatable: false;
}

impl LonghandId {
    pub fn def(self) -> &'static PropertyDef {
        &PROPERTIES[self as usize]
    }

    pub fn name(self) -> &'static str {
        self.def().name
    }

    pub fn inherited(self) -> bool {
        self.def().inherited
    }
}

/// Busca un longhand por nombre (ASCII case-insensitive).
pub fn longhand_by_name(name: &str) -> Option<LonghandId> {
    static INDEX: OnceLock<HashMap<&'static str, LonghandId>> = OnceLock::new();
    let index = INDEX.get_or_init(|| PROPERTIES.iter().map(|p| (p.name, p.id)).collect());
    index.get(name.to_ascii_lowercase().as_str()).copied()
}

/// CSS-wide keywords (CSS Cascade 5 §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssWide {
    Initial,
    Inherit,
    Unset,
    Revert,
    RevertLayer,
}

impl CssWide {
    pub fn parse(input: &[ComponentValue]) -> Option<CssWide> {
        Some(
            match crate::values::single_ident(input)?
                .to_ascii_lowercase()
                .as_str()
            {
                "initial" => CssWide::Initial,
                "inherit" => CssWide::Inherit,
                "unset" => CssWide::Unset,
                "revert" => CssWide::Revert,
                "revert-layer" => CssWide::RevertLayer,
                _ => return None,
            },
        )
    }
}

/// Valores iniciales computados (con un contexto por defecto: 16px, sin padre).
pub fn initial_values() -> &'static [Value] {
    static INITIAL: OnceLock<Vec<Value>> = OnceLock::new();
    INITIAL.get_or_init(|| {
        let ctx = ComputeContext {
            len: LengthContext::default(),
            parent: None,
        };
        PROPERTIES
            .iter()
            .map(|p| {
                let tokens = bf_css::syntax::parse_component_value_list(p.initial);
                let value = (p.parse)(crate::values::trim(&tokens))
                    .unwrap_or_else(|| panic!("valor inicial inválido para {}", p.name));
                (p.compute)(&value, &ctx)
            })
            .collect()
    })
}

impl ComputedStyle {
    /// El estilo con todos los valores iniciales.
    pub fn initial() -> &'static ComputedStyle {
        static STYLE: OnceLock<ComputedStyle> = OnceLock::new();
        STYLE.get_or_init(|| build_initial_style(initial_values()))
    }

    pub fn get(&self, id: LonghandId) -> Value {
        (id.def().get)(self)
    }

    pub fn set(&mut self, value: Value) {
        (value.id().def().set)(self, value);
    }

    /// Valor computado serializado de una propiedad (por nombre).
    pub fn get_css(&self, name: &str) -> Option<String> {
        if name.starts_with("--") {
            return self.custom.get(name).map(|v| {
                bf_css::stylesheet::component_values_to_string(v)
                    .trim()
                    .to_owned()
            });
        }
        let id = longhand_by_name(name)?;
        Some((id.def().to_css)(&self.get(id)))
    }

    pub fn font_size_px(&self) -> f32 {
        self.font_size.px()
    }

    /// Anchos de borde usados (arriba, derecha, abajo, izquierda): 0 en los lados con estilo
    /// `none` o `hidden`. El valor computado no depende del estilo (CSS Backgrounds 3 §4.3,
    /// resolución del CSSWG de 2023), así `border-width: inherit` hereda el ancho declarado.
    pub fn used_border_widths(&self) -> [f32; 4] {
        let used = |w: BorderWidth, s: BorderStyle| {
            if matches!(s, BorderStyle::None | BorderStyle::Hidden) {
                0.0
            } else {
                w.px()
            }
        };
        [
            used(self.border_top_width, self.border_top_style),
            used(self.border_right_width, self.border_right_style),
            used(self.border_bottom_width, self.border_bottom_style),
            used(self.border_left_width, self.border_left_style),
        ]
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle::initial().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_consistent() {
        for (i, p) in PROPERTIES.iter().enumerate() {
            assert_eq!(p.id as usize, i, "{}", p.name);
            assert_eq!(longhand_by_name(p.name), Some(p.id));
        }
        let initial = ComputedStyle::initial();
        assert_eq!(initial.font_size_px(), 16.0);
        assert_eq!(initial.display, Display::Inline);
        // `medium` (3px) computa a 0 porque el estilo inicial es `none`.
        // `medium`; el ancho usado es 0 porque el estilo inicial es `none`.
        assert_eq!(initial.get_css("border-top-width").as_deref(), Some("3px"));
        assert_eq!(initial.used_border_widths(), [0.0; 4]);
        assert_eq!(initial.get_css("color").as_deref(), Some("rgb(0, 0, 0)"));
    }
}
