//! Lista de visualización: las operaciones de pintura en orden, en coordenadas de página (px
//! CSS). El constructor ([`crate::builder`]) la arma desde el árbol de fragmentos siguiendo
//! CSS 2.2 Apéndice E; el rasterizador ([`crate::raster`]) la ejecuta.

use std::sync::Arc;

use bf_layout::fragment::{Edges, Rect, Size};
use bf_layout::scroll::StickyConstraints;
use bf_layout::{FontKey, Outline, Widget};
use bf_style::properties::types::{BorderStyle, TextDecorationStyle};
use bf_style::values::color::Rgba;

/// Radios (horizontal, vertical) de las esquinas: arriba-izquierda, arriba-derecha,
/// abajo-derecha y abajo-izquierda.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Radii(pub [(f32, f32); 4]);

impl Radii {
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&(h, v)| h <= 0.0 || v <= 0.0)
    }

    /// Escala los radios para que los de un mismo lado no se solapen (CSS Backgrounds 3
    /// §5.5).
    pub fn fit(mut self, rect: &Rect) -> Radii {
        let [tl, tr, br, bl] = self.0;
        let ratio = |len: f32, sum: f32| if sum > 0.0 { len / sum } else { f32::INFINITY };
        let f = ratio(rect.width, tl.0 + tr.0)
            .min(ratio(rect.width, bl.0 + br.0))
            .min(ratio(rect.height, tl.1 + bl.1))
            .min(ratio(rect.height, tr.1 + br.1));
        if f < 1.0 {
            for r in &mut self.0 {
                r.0 *= f;
                r.1 *= f;
            }
        }
        self
    }

    /// Radios de la curva interior (padding box) dados los anchos de borde (§5.6).
    pub fn shrink(&self, e: &Edges) -> Radii {
        let [tl, tr, br, bl] = self.0;
        let s = |r: f32, w: f32| (r - w).max(0.0);
        Radii([
            (s(tl.0, e.left), s(tl.1, e.top)),
            (s(tr.0, e.right), s(tr.1, e.top)),
            (s(br.0, e.right), s(br.1, e.bottom)),
            (s(bl.0, e.left), s(bl.1, e.bottom)),
        ])
    }
}

/// Un glifo a dibujar, relativo al origen del texto (la baseline).
#[derive(Debug, Clone)]
pub struct GlyphInstance {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub outline: Option<Arc<Outline>>,
}

#[derive(Debug, Clone)]
pub enum DisplayItem {
    /// Relleno (fondos). `radii` cero es un rectángulo.
    Rect {
        rect: Rect,
        color: Rgba,
        radii: Radii,
    },
    /// Bordes de una border box. Orden de lados: arriba, derecha, abajo, izquierda.
    Border {
        rect: Rect,
        widths: Edges,
        colors: [Rgba; 4],
        styles: [BorderStyle; 4],
        radii: Radii,
    },
    /// Texto de una sola cara.
    Text {
        glyphs: Vec<GlyphInstance>,
        font: FontKey,
        size: f32,
        color: Rgba,
        origin: (f32, f32),
        /// Escalas verticales del contorno sobre y bajo la baseline (ver
        /// `FontProvider::vertical_fit`).
        y_fit: (f32, f32),
    },
    /// Una imagen escalada a `rect` (contenido de `<img>`, marcadores).
    Image {
        rect: Rect,
        image: Arc<tiny_skia::Pixmap>,
    },
    /// Una capa de `background-image`: el área pintada (con sus radios), el primer tile y cómo
    /// se repite en cada eje. `xs`/`ys` son las posiciones de los tiles en los ejes que no se
    /// repiten indefinidamente (`no-repeat`, `space`); `None` = se repite en todo el área.
    BackgroundImage {
        area: Rect,
        radii: Radii,
        tile: Rect,
        xs: Option<Vec<f32>>,
        ys: Option<Vec<f32>>,
        image: Arc<tiny_skia::Pixmap>,
    },
    /// Una capa de `background-image` que es un `<gradient>` (CSS Images 3 §3): como
    /// [`DisplayItem::BackgroundImage`], pero el tile se genera a resolución de dispositivo.
    /// La geometría de `gradient` está en coordenadas del tile (origen en su esquina).
    Gradient {
        area: Rect,
        radii: Radii,
        tile: Rect,
        xs: Option<Vec<f32>>,
        ys: Option<Vec<f32>>,
        gradient: Arc<ResolvedGradient>,
    },
    /// Una sombra de `box-shadow` (CSS Backgrounds 3 §7.1): `rect` y `radii` son la border box
    /// (exterior) o la padding box (`inset`) del elemento, que recortan la sombra.
    BoxShadow {
        rect: Rect,
        radii: Radii,
        shadow: BoxShadowParams,
    },
    /// Abre un grupo que se pinta como sombra desenfocada (`text-shadow`, CSS Text Decoration
    /// 3 §4): lo que sigue hasta [`DisplayItem::PopShadow`] se pinta en una capa aparte (ya
    /// desplazado y en el color de la sombra), se desenfoca con σ = `blur` / 2 y se compone.
    /// `bounds` acota (en px CSS) lo que se pinta dentro del grupo, sin el desenfoque.
    PushShadow {
        bounds: Rect,
        blur: f32,
    },
    PopShadow,
    /// Parte nativa de un control de formulario en `rect`; `color` es el del texto del
    /// control.
    Widget {
        rect: Rect,
        widget: Widget,
        color: Rgba,
    },
    /// Un trazo vectorial en px CSS (iconos de la interfaz): relleno opcional y contorno de
    /// `stroke.0` px con extremos y uniones redondeados.
    Path {
        ops: Vec<PathOp>,
        fill: Option<Rgba>,
        stroke: Option<(f32, Rgba)>,
    },
    /// Decoración de texto horizontal: `rect` es la banda de la línea.
    Line {
        rect: Rect,
        color: Rgba,
        style: TextDecorationStyle,
    },
    PushClip {
        rect: Rect,
        radii: Radii,
    },
    PopClip,
    /// Recorte al área que pinta un borde con esos anchos, estilos y radios (`background-clip:
    /// border-area`, CSS Backgrounds 4 §2.1); se cierra con [`DisplayItem::PopClip`].
    PushClipBorder {
        rect: Rect,
        widths: Edges,
        styles: [BorderStyle; 4],
        radii: Radii,
    },
    /// Recorte a la forma de los glifos de `glyphs` (ítems `Text`) (`background-clip: text`,
    /// CSS Backgrounds 4 §2.1); se cierra con [`DisplayItem::PopClip`].
    PushClipText {
        glyphs: Vec<DisplayItem>,
    },
    PushOpacity(f32),
    PopOpacity,
    /// Abre una capa enmascarada (`mask-image`, CSS Masking 1 §7) (E7).
    PushMask,
    /// Cierra la capa de [`DisplayItem::PushMask`]: `layers` (capas de imagen o gradiente, con
    /// sus recortes) se pintan aparte y su alfa recorta la capa al componerla.
    PopMask {
        layers: Vec<DisplayItem>,
    },
    PushTransform {
        dx: f32,
        dy: f32,
    },
    /// E4: una transformación afín 2D (CSS Transforms 1 §6), `[a, b, c, d, e, f]` como en
    /// `matrix()`: lo que sigue hasta [`DisplayItem::PopTransform`] se pinta con ella
    /// (multiplicada por la actual).
    PushMatrix([f32; 6]),
    PopTransform,
    /// Contenido de una caja con `position: fixed`: se pinta respecto del viewport, sin el
    /// desplazamiento del scroll.
    PushFixed,
    PopFixed,
    /// E7: abre la capa de un elemento con `filter`, `clip-path` o `mix-blend-mode`.
    PushEffect,
    /// E7: cierra la capa de [`DisplayItem::PushEffect`] aplicándole el efecto (ver
    /// [`crate::filters`]).
    PopEffect(Box<crate::filters::Effect>),
    /// E4 (pedido de E12): contenido de una caja `position: sticky` cuyo scrollport es el
    /// viewport (CSS Position 3 §3.4). Los fragmentos ya traen `applied`, el desplazamiento
    /// calculado con el scroll de la página de la maqueta; el raster recalcula el desplazamiento
    /// con su scroll (`constraints.offset` contra el rectángulo de vista: scroll de la página
    /// y `viewport`) y traslada lo que sigue hasta [`DisplayItem::PopSticky`] por la
    /// diferencia. Así desplazar la página no obliga a rehacer la lista.
    PushSticky {
        constraints: StickyConstraints,
        applied: (f32, f32),
        viewport: Size,
    },
    PopSticky,
}

/// Parámetros de una sombra de caja ya resueltos a px CSS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxShadowParams {
    pub offset: (f32, f32),
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
    pub inset: bool,
}

/// Forma de un gradiente resuelto, en px CSS relativos al origen del tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientShape {
    /// La línea del gradiente va de `start` (0) a `end` (1) (CSS Images 3 §3.1.1).
    Linear { start: (f32, f32), end: (f32, f32) },
    /// Elipse de radios `radii` centrada en `center`; 1 = la forma final (§3.2.1).
    Radial {
        center: (f32, f32),
        radii: (f32, f32),
    },
    /// Ángulo inicial `from` en grados (0 = arriba, sentido horario) (CSS Images 4 §3.3).
    Conic { center: (f32, f32), from: f32 },
}

/// Una parada ya corregida (CSS Images 3 §3.4.3): posición como fracción de la línea del
/// gradiente (o de la vuelta, en los cónicos), no decreciente.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Rgba,
    /// Indicación de interpolación hacia la parada siguiente, como fracción del tramo
    /// (`None` = punto medio, interpolación lineal).
    pub hint: Option<f32>,
}

/// Un gradiente listo para rasterizar, independiente de `bf_style`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedGradient {
    pub shape: GradientShape,
    pub stops: Vec<GradientStop>,
    pub repeating: bool,
}

/// Comando de un [`DisplayItem::Path`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathOp {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

impl DisplayList {
    pub fn push(&mut self, item: DisplayItem) {
        self.items.push(item);
    }
}

// --- E4: lo que depende del scroll (desplazamiento incremental y raster por bandas) ---

/// Un grupo `sticky` del viewport: su rectángulo (px CSS de página, sin el desplazamiento
/// `sticky`) y lo que hace falta para saber dónde se pinta con cada scroll.
#[derive(Debug, Clone)]
pub struct StickyGroup {
    pub rect: Rect,
    pub constraints: StickyConstraints,
    pub applied: (f32, f32),
    pub viewport: Size,
    /// Los índices de sus ítems en la lista, de `PushSticky` a `PopSticky` incluidos.
    pub items: std::ops::Range<usize>,
    /// La traslación de la página en `PushSticky` (px CSS; `rect` ya la incluye).
    pub translate: (f32, f32),
    /// Solo está dentro de recortes rectangulares sin esquinas redondeadas (no de una capa de
    /// opacidad, una máscara, un efecto ni una sombra): se puede pintar en su propia capa y
    /// componerla encima de la página, recortada a `clip`.
    pub own_layer: bool,
    /// La intersección de los recortes que lo encierran, en px CSS de la página (no se mueven
    /// con el desplazamiento `sticky`).
    pub clip: Option<Rect>,
}

/// Lo que necesitan el desplazamiento incremental y el raster por bandas
/// ([`crate::raster::scroll_frame`], [`crate::raster::render_parallel`]).
#[derive(Debug, Clone, Default)]
pub struct ScrollInfo {
    /// Cuántos px CSS alrededor de un tile hay que rasterizar para que los desenfoques se vean
    /// continuos de un tile al siguiente (leen píxeles vecinos).
    pub margin: f32,
    /// El desenfoque más grande de la lista (px CSS; 0 si no hay).
    pub blur: f32,
    /// Lo que pinta cada grupo `fixed`, en px CSS del viewport (no se mueve con el scroll).
    pub fixed: Vec<Rect>,
    /// Los grupos `sticky` del viewport (se mueven distinto que la página).
    pub sticky: Vec<StickyGroup>,
    /// Lo que pintan los ítems de fuera de los grupos `sticky` que van detrás del primero en
    /// la lista: índice, caja conservadora (px CSS de la página, o del viewport si `true`:
    /// dentro de un grupo `fixed`) y la transformación del ítem (para [`ink_bounds`]). Si algo
    /// de eso cae sobre un `sticky`, el `sticky` no va en capa propia.
    pub after_sticky: Vec<(usize, Rect, bool, [f32; 6])>,
}

/// Una transformación afín `[a, b, c, d, e, f]` (como `matrix()`).
type Affine = [f32; 6];

const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn concat(m: &Affine, n: &Affine) -> Affine {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn map_rect(m: &Affine, r: &Rect) -> Rect {
    let pts = [
        (r.x, r.y),
        (r.x + r.width, r.y),
        (r.x, r.y + r.height),
        (r.x + r.width, r.y + r.height),
    ]
    .map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => {
            let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
            let x1 = (a.x + a.width).max(b.x + b.width);
            let y1 = (a.y + a.height).max(b.y + b.height);
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }
}

fn inflate(r: &Rect, d: f32) -> Rect {
    Rect::new(r.x - d, r.y - d, r.width + 2.0 * d, r.height + 2.0 * d)
}

/// Lo que puede pintar `item` (px CSS en sus coordenadas), con margen para el antialiasing.
/// `None`: no pinta nada por sí mismo (recortes, capas, transformaciones).
pub(crate) fn item_bounds(item: &DisplayItem) -> Option<Rect> {
    let r = match item {
        DisplayItem::Rect { rect, .. }
        | DisplayItem::Border { rect, .. }
        | DisplayItem::Image { rect, .. }
        | DisplayItem::Widget { rect, .. }
        | DisplayItem::Line { rect, .. } => *rect,
        DisplayItem::BackgroundImage { area, .. } | DisplayItem::Gradient { area, .. } => *area,
        DisplayItem::BoxShadow { rect, shadow, .. } => inflate(
            rect,
            shadow.blur * 2.0
                + shadow.spread.abs()
                + shadow.offset.0.abs().max(shadow.offset.1.abs()),
        ),
        DisplayItem::Text {
            glyphs,
            size,
            origin,
            ..
        } => {
            let (mut x0, mut x1, mut y0, mut y1) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for g in glyphs {
                x0 = x0.min(g.x);
                x1 = x1.max(g.x);
                y0 = y0.min(g.y);
                y1 = y1.max(g.y);
            }
            Rect::new(
                origin.0 + x0 - size,
                origin.1 + y0 - 2.0 * size,
                x1 - x0 + 3.0 * size,
                y1 - y0 + 3.0 * size,
            )
        }
        DisplayItem::Path { ops, stroke, .. } => {
            let mut b: Option<Rect> = None;
            for op in ops {
                let pts: &[(f32, f32)] = &match *op {
                    PathOp::MoveTo(x, y) | PathOp::LineTo(x, y) => vec![(x, y)],
                    PathOp::QuadTo(a, b, x, y) => vec![(a, b), (x, y)],
                    PathOp::CubicTo(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
                    PathOp::Close => vec![],
                };
                for &(x, y) in pts {
                    b = Some(union(b, Rect::new(x, y, 0.0, 0.0)));
                }
            }
            inflate(&b?, stroke.map_or(0.0, |s| s.0))
        }
        DisplayItem::PushShadow { bounds, blur } => inflate(bounds, blur * 2.0),
        _ => return None,
    };
    Some(inflate(&r, 2.0))
}

/// E4: la caja de lo que pinta `item` con la transformación `m` (px CSS), más ajustada que
/// [`item_bounds`] para el texto: la de los puntos de los contornos de sus glifos (encierran
/// la curva), con 2 px de margen por el ajuste a la rejilla y el antialiasing. Los glifos de
/// mapa de bits y el resto de los ítems usan [`item_bounds`].
pub fn ink_bounds(item: &DisplayItem, m: &[f32; 6]) -> Option<Rect> {
    let DisplayItem::Text {
        glyphs,
        size,
        origin,
        y_fit,
        ..
    } = item
    else {
        return item_bounds(item).map(|b| map_rect(m, &b));
    };
    let mut b: Option<Rect> = None;
    for g in glyphs {
        let Some(o) = &g.outline else {
            continue;
        };
        if !o.bitmaps.is_empty() {
            return item_bounds(item).map(|b| map_rect(m, &b));
        }
        let k = size / o.units_per_em;
        for cmd in &o.commands {
            let pts: &[(f32, f32)] = &match *cmd {
                bf_layout::PathCommand::MoveTo(x, y) | bf_layout::PathCommand::LineTo(x, y) => {
                    vec![(x, y)]
                }
                bf_layout::PathCommand::QuadTo(a, b, x, y) => vec![(a, b), (x, y)],
                bf_layout::PathCommand::CurveTo(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
                bf_layout::PathCommand::Close => vec![],
            };
            for &(x, y) in pts {
                let fit = if y >= 0.0 { y_fit.0 } else { y_fit.1 };
                let p = Rect::new(
                    origin.0 + g.x + x * k,
                    origin.1 + g.y - y * k * fit,
                    0.0,
                    0.0,
                );
                b = Some(union(b, p));
            }
        }
    }
    // Sin contornos no pinta nada: una caja vacía no toca nada.
    let b = b.map_or(Rect::new(origin.0, origin.1, 0.0, 0.0), |r| {
        inflate(&r, 2.0)
    });
    Some(map_rect(m, &b))
}

/// Analiza la lista para el raster en tiles y el desplazamiento incremental
/// ([`crate::raster::render_tiled`]). `None` si algo lo impide: un `filter` (puede leer
/// cualquier píxel de su capa, así que un tile no basta), un grupo `fixed` o `sticky` dentro
/// de otro, o un `sticky` bajo una transformación que no es una traslación (no se sabe dónde
/// queda con otro scroll). Esas listas se rasterizan enteras, de una pasada.
pub fn scroll_info(dl: &DisplayList) -> Option<ScrollInfo> {
    enum Group {
        Fixed,
        Sticky(StickyConstraints, (f32, f32), Size),
    }
    let mut info = ScrollInfo::default();
    let mut blur: f32 = 0.0;
    let mut stack: Vec<Affine> = vec![IDENTITY];
    // El grupo abierto: su clase, la profundidad de la pila al abrirlo y lo que pinta.
    let mut group: Option<(Group, usize, Option<Rect>)> = None;
    // Dónde abrió el grupo `sticky` en curso: índice, traslación, si puede ir en capa propia y
    // el recorte que lo encierra.
    let mut opened = (0usize, (0.0f32, 0.0f32), false, None::<Rect>);
    // Recortes, capas, máscaras, efectos y sombras abiertos: el rectángulo (px CSS de la
    // página) de los recortes rectangulares bajo una traslación, `None` el resto.
    let mut scopes: Vec<Option<Rect>> = Vec::new();
    for (index, item) in dl.items.iter().enumerate() {
        let top = *stack.last().unwrap_or(&IDENTITY);
        let translation = top[0] == 1.0 && top[1] == 0.0 && top[2] == 0.0 && top[3] == 1.0;
        match item {
            DisplayItem::PushClip { rect, radii } => {
                scopes.push((radii.is_zero() && translation).then(|| map_rect(&top, rect)))
            }
            DisplayItem::PushClipBorder { .. }
            | DisplayItem::PushClipText { .. }
            | DisplayItem::PushOpacity(_)
            | DisplayItem::PushMask
            | DisplayItem::PushEffect
            | DisplayItem::PushShadow { .. } => scopes.push(None),
            DisplayItem::PopClip
            | DisplayItem::PopOpacity
            | DisplayItem::PopMask { .. }
            | DisplayItem::PopEffect(_)
            | DisplayItem::PopShadow => {
                scopes.pop();
            }
            _ => {}
        }
        match item {
            DisplayItem::PopEffect(fx) if fx.filter.is_some() => return None,
            DisplayItem::BoxShadow { shadow, .. } => blur = blur.max(shadow.blur),
            DisplayItem::PushShadow { blur: b, .. } => blur = blur.max(*b),
            _ => {}
        }
        match item {
            DisplayItem::PushTransform { dx, dy } => {
                stack.push(concat(&top, &[1.0, 0.0, 0.0, 1.0, *dx, *dy]));
            }
            DisplayItem::PushMatrix(m) => stack.push(concat(&top, m)),
            DisplayItem::PushFixed => {
                if group.is_some() {
                    return None;
                }
                stack.push(IDENTITY);
                group = Some((Group::Fixed, stack.len(), None));
            }
            DisplayItem::PushSticky {
                constraints,
                applied,
                viewport,
            } => {
                if group.is_some() || top[0] != 1.0 || top[1] != 0.0 || top[2] != 0.0 {
                    return None;
                }
                if top[3] != 1.0 {
                    return None;
                }
                stack.push(top);
                let own = scopes.iter().all(Option::is_some);
                let clip = scopes.iter().flatten().fold(None, |acc: Option<Rect>, r| {
                    Some(acc.map_or(*r, |a| {
                        let (x0, y0) = (a.x.max(r.x), a.y.max(r.y));
                        let x1 = (a.x + a.width).min(r.x + r.width);
                        let y1 = (a.y + a.height).min(r.y + r.height);
                        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
                    }))
                });
                opened = (index, (top[4], top[5]), own, clip);
                group = Some((
                    Group::Sticky(*constraints, *applied, *viewport),
                    stack.len(),
                    None,
                ));
            }
            DisplayItem::PopTransform | DisplayItem::PopFixed | DisplayItem::PopSticky => {
                if let Some((_, depth, _)) = &group
                    && *depth == stack.len()
                {
                    let (kind, _, rect) = group.take().expect("grupo abierto");
                    if let Some(rect) = rect {
                        match kind {
                            Group::Fixed => info.fixed.push(rect),
                            Group::Sticky(constraints, applied, viewport) => {
                                info.sticky.push(StickyGroup {
                                    rect,
                                    constraints,
                                    applied,
                                    viewport,
                                    items: opened.0..index + 1,
                                    translate: opened.1,
                                    own_layer: opened.2,
                                    clip: opened.3,
                                });
                            }
                        }
                    }
                }
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            other => {
                let Some(b) = item_bounds(other) else {
                    continue;
                };
                let b = map_rect(&top, &b);
                match &mut group {
                    Some((Group::Sticky(..), _, acc)) => *acc = Some(union(*acc, b)),
                    Some((Group::Fixed, _, acc)) => {
                        *acc = Some(union(*acc, b));
                        if !info.sticky.is_empty() {
                            info.after_sticky.push((index, b, true, top));
                        }
                    }
                    None if !info.sticky.is_empty() => {
                        info.after_sticky.push((index, b, false, top));
                    }
                    None => {}
                }
            }
        }
    }
    if group.is_some() {
        return None;
    }
    // σ = blur / 2: tres σ (1,5 × blur) alcanzan para el desenfoque; el resto cubre el
    // antialiasing de lo que la banda corta y las regiones de sombra recortadas al lienzo
    // (medido: hasta 15 filas con un `text-shadow` de 3 px).
    info.margin = blur * 3.0 + 16.0;
    info.blur = blur;
    Some(info)
}
