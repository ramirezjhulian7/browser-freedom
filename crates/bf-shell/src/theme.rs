//! Identidad visual de la interfaz del navegador: la paleta (oscura y clara, con el mismo acento
//! ámbar) y la tipografía. Sale del logo: el ámbar de su trazo es el único acento y la tinta
//! azul-negra de su fondo es la base del tema oscuro; el claro es la misma familia en papel. Todo color del cromo, de la superposición de depuración y de las
//! páginas internas sale de aquí; las plantillas HTML los reciben como custom properties
//! (`var(--accent)`, ...) mediante [`Theme::css_vars`].

use bf_style::values::color::Rgba;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// El marco de la ventana: la franja de pestañas y la línea de estado, un tono más hondo
    /// que el cromo para que la pestaña activa se una a la barra.
    pub frame: Rgba,
    /// Fondo del cromo (barra de herramientas, pestaña activa, panel, páginas internas).
    pub chrome: Rgba,
    /// Superficies sobre el cromo (campo de URL, tarjetas).
    pub surface: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    /// El único acento: ámbar.
    pub accent: Rgba,
    /// El acento como texto (enlaces): en el claro, un ámbar tostado que cumple contraste AA.
    pub accent_text: Rgba,
    /// El acento muy diluido: halo del campo con foco, fondos de chips y de la entrada activa.
    pub accent_soft: Rgba,
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

const fn amber(a: f32) -> Rgba {
    Rgba {
        r: 245.0,
        g: 165.0,
        b: 36.0,
        a,
    }
}

/// Tamaños de fuente del cromo (px) y la familia: la fuente de interfaz del sistema.
pub const FONT_FAMILY: &str = "system-ui";
pub const CHROME_FONT_PX: f32 = 13.0;
pub const STATUS_FONT_PX: f32 = 12.0;

impl Theme {
    pub const DARK: Theme = Theme {
        dark: true,
        frame: rgb(0x0E, 0x11, 0x17),
        chrome: rgb(0x16, 0x1A, 0x22),
        surface: rgb(0x1D, 0x22, 0x2C),
        text: rgb(0xEC, 0xEE, 0xF3),
        text_secondary: rgb(0x8D, 0x94, 0xA6),
        accent: AMBER,
        accent_text: AMBER,
        accent_soft: amber(0.14),
        on_accent: rgb(0x12, 0x15, 0x1C),
        border: rgb(0x27, 0x2D, 0x3A),
        hover: rgb(0x22, 0x28, 0x34),
        selection: amber(0.32),
        overlay_block: AMBER,
        overlay_inline: rgb(0x3F, 0xB9, 0x50),
        overlay_text: rgb(0x8D, 0x94, 0xA6),
        danger: rgb(0xF4, 0x6B, 0x5E),
    };

    pub const LIGHT: Theme = Theme {
        dark: false,
        frame: rgb(0xE8, 0xEA, 0xEF),
        chrome: rgb(0xF7, 0xF8, 0xFA),
        surface: rgb(0xFF, 0xFF, 0xFF),
        text: rgb(0x17, 0x1A, 0x21),
        text_secondary: rgb(0x5E, 0x65, 0x74),
        accent: AMBER,
        accent_text: rgb(0x9A, 0x5B, 0x00),
        accent_soft: amber(0.18),
        on_accent: rgb(0x17, 0x1A, 0x21),
        border: rgb(0xDC, 0xDF, 0xE6),
        hover: rgb(0xE6, 0xE8, 0xEE),
        selection: amber(0.35),
        overlay_block: AMBER,
        overlay_inline: rgb(0x1A, 0x7F, 0x37),
        overlay_text: rgb(0x5E, 0x65, 0x74),
        danger: rgb(0xC9, 0x3A, 0x2E),
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
            ":root{{{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}--font:{FONT_FAMILY};--chrome-size:{CHROME_FONT_PX}px;\
             --status-size:{STATUS_FONT_PX}px}}",
            c("frame", self.frame),
            c("chrome", self.chrome),
            c("surface", self.surface),
            c("text", self.text),
            c("text2", self.text_secondary),
            c("accent", self.accent),
            c("accent-text", self.accent_text),
            c("accent-soft", self.accent_soft),
            c("on-accent", self.on_accent),
            c("border", self.border),
            c("hover", self.hover),
            c("selection", self.selection),
            c("inline", self.overlay_inline),
            c("danger", self.danger),
            // El resplandor de la portada: el cromo con una pizca de acento, opaco (un
            // gradiente con extremos semitransparentes deja anillos en el raster).
            c(
                "glow",
                Theme::mix(
                    self.chrome,
                    self.accent,
                    if self.dark { 0.07 } else { 0.10 }
                )
            ),
        )
    }

    /// `a` mezclado con `b` en proporción `t` (0 es `a`), opaco.
    pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
        let m = |x: f32, y: f32| x + (y - x) * t;
        Rgba {
            r: m(a.r, b.r),
            g: m(a.g, b.g),
            b: m(a.b, b.b),
            a: 1.0,
        }
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
