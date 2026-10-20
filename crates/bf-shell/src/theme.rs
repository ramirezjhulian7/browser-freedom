//! Identidad visual de la interfaz del navegador: la paleta (oscura y clara, con el mismo acento
//! ámbar) y la tipografía. Todo color del cromo, de la superposición de depuración y de las
//! páginas internas sale de aquí; las plantillas HTML los reciben como custom properties
//! (`var(--accent)`, ...) mediante [`Theme::css_vars`].

use bf_style::values::color::Rgba;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Fondo del cromo (barra, línea de estado, panel).
    pub chrome: Rgba,
    /// Superficies sobre el cromo (campo de URL, tarjetas).
    pub surface: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    /// El único acento: ámbar.
    pub accent: Rgba,
    /// Texto sobre el acento.
    pub on_accent: Rgba,
    /// Separadores y bordes finos.
    pub border: Rgba,
    /// Fondo de un botón bajo el puntero.
    pub hover: Rgba,
    /// Fondo de la selección de texto del campo de URL.
    pub selection: Rgba,
    /// Superposición de depuración: cajas de bloque (el acento), inline y texto.
    pub overlay_block: Rgba,
    pub overlay_inline: Rgba,
    pub overlay_text: Rgba,
    /// Lo que falló (errores de red, imágenes rotas en el panel).
    pub danger: Rgba,
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba::rgb(r, g, b)
}

const AMBER: Rgba = rgb(0xF5, 0xA5, 0x24);

/// Tamaños de fuente del cromo (px) y la familia: la fuente de interfaz del sistema.
pub const FONT_FAMILY: &str = "system-ui";
pub const CHROME_FONT_PX: f32 = 13.0;
pub const STATUS_FONT_PX: f32 = 12.0;

impl Theme {
    pub const DARK: Theme = Theme {
        dark: true,
        chrome: rgb(0x14, 0x17, 0x1F),
        surface: rgb(0x1E, 0x23, 0x30),
        text: rgb(0xE6, 0xE8, 0xEE),
        text_secondary: rgb(0x8A, 0x92, 0xA6),
        accent: AMBER,
        on_accent: rgb(0x14, 0x17, 0x1F),
        border: rgb(0x2A, 0x30, 0x40),
        hover: rgb(0x28, 0x2E, 0x3D),
        selection: Rgba {
            r: 245.0,
            g: 165.0,
            b: 36.0,
            a: 0.35,
        },
        overlay_block: AMBER,
        overlay_inline: rgb(0x3F, 0xB9, 0x50),
        overlay_text: rgb(0x8A, 0x92, 0xA6),
        danger: rgb(0xF8, 0x51, 0x49),
    };

    pub const LIGHT: Theme = Theme {
        dark: false,
        chrome: rgb(0xF7, 0xF7, 0xF9),
        surface: rgb(0xFF, 0xFF, 0xFF),
        text: rgb(0x1A, 0x1D, 0x26),
        text_secondary: rgb(0x6B, 0x72, 0x80),
        accent: AMBER,
        on_accent: rgb(0x1A, 0x1D, 0x26),
        border: rgb(0xE2, 0xE4, 0xEA),
        hover: rgb(0xEC, 0xED, 0xF1),
        selection: Rgba {
            r: 245.0,
            g: 165.0,
            b: 36.0,
            a: 0.35,
        },
        overlay_block: AMBER,
        overlay_inline: rgb(0x1A, 0x7F, 0x37),
        overlay_text: rgb(0x6B, 0x72, 0x80),
        danger: rgb(0xCF, 0x22, 0x2E),
    };

    /// El tema según el esquema del sistema (`prefers-color-scheme`); `BF_THEME=light|dark`
    /// lo fuerza. Sin información, el oscuro.
    pub fn for_system(system_dark: Option<bool>) -> Theme {
        let forced = std::env::var("BF_THEME").ok();
        let dark = match forced.as_deref() {
            Some("light") => false,
            Some("dark") => true,
            _ => system_dark.unwrap_or(true),
        };
        if dark { Theme::DARK } else { Theme::LIGHT }
    }

    /// Las custom properties de la paleta y la tipografía para las plantillas HTML.
    pub fn css_vars(&self) -> String {
        let c = |name: &str, v: Rgba| format!("--{name}:{};", css(v));
        format!(
            ":root{{{}{}{}{}{}{}{}{}{}{}{}--font:{FONT_FAMILY};--chrome-size:{CHROME_FONT_PX}px;\
             --status-size:{STATUS_FONT_PX}px}}",
            c("chrome", self.chrome),
            c("surface", self.surface),
            c("text", self.text),
            c("text2", self.text_secondary),
            c("accent", self.accent),
            c("on-accent", self.on_accent),
            c("border", self.border),
            c("hover", self.hover),
            c("selection", self.selection),
            c("inline", self.overlay_inline),
            c("danger", self.danger),
        )
    }

    /// `color` con la opacidad multiplicada por `alpha`.
    pub fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
        Rgba {
            a: color.a * alpha,
            ..color
        }
    }
}

/// Serialización CSS de un color.
pub fn css(c: Rgba) -> String {
    c.to_string()
}
