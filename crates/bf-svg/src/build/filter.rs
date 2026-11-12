//! Filtros SVG (Filter Effects Module Level 1): la propiedad `filter` de un elemento (§5), el
//! elemento `filter` con su región y sus unidades (§9.2) y las primitivas (§9.4–§9.18), ya
//! resueltas al sistema de coordenadas del elemento filtrado. El rasterizado es de `bf-paint`.

use std::collections::HashMap;

use bf_style::values::color::parse_color;

use super::{Axis, Bbox, Builder, Len, MAX_DEPTH, Viewport, cv_fraction, one, parse_len, url_of};
use crate::geom;
use crate::style::Node as DocNode;
use crate::{
    BlendMode, CompositeOp, EdgeMode, Filter, FilterInput, FilterKind, FilterPrimitive, Rgba,
};

/// Primitivas de filtro (§9): cualquier otro hijo del elemento `filter` se ignora.
const PRIMITIVES: &[&str] = &[
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDropShadow",
    "feFlood",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMorphology",
    "feOffset",
    "feSpecularLighting",
    "feTile",
    "feTurbulence",
];

/// Matriz identidad de `feColorMatrix` (4×5 por filas).
const IDENTITY: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// Une dos rectángulos (x, y, ancho, alto).
fn union(a: Bbox, b: Bbox) -> Bbox {
    let x0 = a.0.min(b.0);
    let y0 = a.1.min(b.1);
    let x1 = (a.0 + a.2).max(b.0 + b.2);
    let y1 = (a.1 + a.3).max(b.1 + b.3);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Un número de atributo (`dx`, `k1`, …).
fn number(n: &DocNode, name: &str) -> Option<f32> {
    match geom::parse_numbers(n.attr(name)?)?.as_slice() {
        [v] if v.is_finite() => Some(*v),
        _ => None,
    }
}

/// Matriz de `feColorMatrix` (§9.6) según `type` y `values`. Una lista de valores con un
/// número de entradas incorrecto deja pasar la imagen (identidad).
fn color_matrix(n: &DocNode) -> [f32; 20] {
    let values = n.attr("values").and_then(geom::parse_numbers);
    let ty = n.attr("type").map(str::trim).unwrap_or("matrix");
    match ty {
        "saturate" => {
            let s = match values.as_deref() {
                None => 1.0,
                Some([s]) if *s >= 0.0 => *s,
                Some(_) => return IDENTITY,
            };
            [
                0.213 + 0.787 * s,
                0.715 - 0.715 * s,
                0.072 - 0.072 * s,
                0.0,
                0.0,
                0.213 - 0.213 * s,
                0.715 + 0.285 * s,
                0.072 - 0.072 * s,
                0.0,
                0.0,
                0.213 - 0.213 * s,
                0.715 - 0.715 * s,
                0.072 + 0.928 * s,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ]
        }
        "hueRotate" => {
            let deg = match values.as_deref() {
                None => 0.0,
                Some([a]) => *a,
                Some(_) => return IDENTITY,
            };
            let (s, c) = deg.to_radians().sin_cos();
            [
                0.213 + c * 0.787 - s * 0.213,
                0.715 - c * 0.715 - s * 0.715,
                0.072 - c * 0.072 + s * 0.928,
                0.0,
                0.0,
                0.213 - c * 0.213 + s * 0.143,
                0.715 + c * 0.285 + s * 0.140,
                0.072 - c * 0.072 - s * 0.283,
                0.0,
                0.0,
                0.213 - c * 0.213 - s * 0.787,
                0.715 - c * 0.715 + s * 0.715,
                0.072 + c * 0.928 + s * 0.072,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
            ]
        }
        "luminanceToAlpha" => {
            let mut m = [0.0; 20];
            m[15] = 0.2126;
            m[16] = 0.7152;
            m[17] = 0.0722;
            m
        }
        // `matrix` y valores no válidos de `type` (su valor inicial es `matrix`).
        _ => match values {
            Some(v) if v.len() == 20 => {
                let mut m = [0.0; 20];
                m.copy_from_slice(&v);
                m
            }
            _ => IDENTITY,
        },
    }
}

impl Builder<'_> {
    /// El elemento `filter` que pide la propiedad `filter` de `n`. `none`, las funciones de
    /// filtro de CSS (sin soporte) y una referencia a algo que no existe o no es un `filter`
    /// dan `None`: «the whole filter chain is ignored. No filter is applied to the object»
    /// (Filter Effects 1 §5).
    pub(super) fn filter_ref(&self, n: &DocNode) -> Option<usize> {
        let v = n.props.get("filter")?;
        let url = one(v).and_then(url_of)?;
        self.by_id(&url).filter(|&f| self.node(f).name == "filter")
    }

    /// La cadena `href` de un `filter` (§9.2): el propio elemento y los que hereda.
    fn filter_chain(&self, f: usize) -> Vec<usize> {
        let mut chain = vec![f];
        while chain.len() < MAX_DEPTH {
            let last = self.node(*chain.last().unwrap_or(&f));
            let Some(next) = last
                .attr("href")
                .and_then(|h| self.by_id(h))
                .filter(|&n| self.node(n).name == "filter" && !chain.contains(&n))
            else {
                break;
            };
            chain.push(next);
        }
        chain
    }

    /// El padre de un nodo del documento (el árbol solo guarda los hijos).
    fn parent_of(&self, i: usize) -> Option<usize> {
        self.svg.nodes.iter().position(|n| n.children.contains(&i))
    }

    /// `color-interpolation-filters` (§10, heredada, inicial `linearRGB`) de la primitiva
    /// `prim` hija del `filter` `owner`: se busca en la primitiva y en sus ancestros. `auto`
    /// se trata como sRGB.
    fn linear_rgb(&self, prim: usize, owner: usize) -> bool {
        let mut cur = Some(prim);
        let mut steps = 0;
        while let Some(i) = cur {
            if let Some(v) = self.node(i).props.get("color-interpolation-filters")
                && let Some(t) = super::ident(v)
            {
                match t.as_str() {
                    "linearrgb" => return true,
                    "srgb" | "auto" => return false,
                    _ => {}
                }
            }
            steps += 1;
            if steps > 256 {
                break;
            }
            cur = if i == prim {
                Some(owner)
            } else {
                self.parent_of(i)
            };
        }
        true
    }

    /// Resuelve el `filter` `f` para un elemento con caja `bbox` (en su sistema de usuario).
    /// `None` = el elemento no se pinta: el filtro no tiene primitivas (§9.3) o su región
    /// queda vacía (una región de ancho o alto no positivo, o `objectBoundingBox` sin caja).
    pub(super) fn filter(&self, f: usize, vp: Viewport, bbox: Option<Bbox>) -> Option<Filter> {
        let chain = self.filter_chain(f);
        let attr = |name: &str| chain.iter().find_map(|&c| self.node(c).attr(name));
        let fnode = self.node(f);
        let fst = self.computed(fnode, &self.resource_st(), vp);
        let bbox = bbox.filter(|b| b.2 > 0.0 && b.3 > 0.0);
        // `filterUnits` (inicial `objectBoundingBox`) y `primitiveUnits` (inicial
        // `userSpaceOnUse`), §9.2.
        let filter_obb = attr("filterUnits").is_none_or(|u| u.trim() != "userSpaceOnUse");
        let prim_obb = attr("primitiveUnits").is_some_and(|u| u.trim() == "objectBoundingBox");
        let user_len = |s: Option<&str>, axis: Axis| -> Option<f32> {
            let l = parse_len(s?)?;
            Some(vp.resolve(l, axis, fst.size))
        };
        let region = if filter_obb {
            let (bx, by, bw, bh) = bbox?;
            (
                bx + Self::bbox_fraction(attr("x"), -0.1) * bw,
                by + Self::bbox_fraction(attr("y"), -0.1) * bh,
                Self::bbox_fraction(attr("width"), 1.2) * bw,
                Self::bbox_fraction(attr("height"), 1.2) * bh,
            )
        } else {
            let pct = |p: f32, axis: Axis| vp.resolve(Len::Pct(p), axis, fst.size);
            (
                user_len(attr("x"), Axis::X).unwrap_or_else(|| pct(-10.0, Axis::X)),
                user_len(attr("y"), Axis::Y).unwrap_or_else(|| pct(-10.0, Axis::Y)),
                user_len(attr("width"), Axis::X).unwrap_or_else(|| pct(120.0, Axis::X)),
                user_len(attr("height"), Axis::Y).unwrap_or_else(|| pct(120.0, Axis::Y)),
            )
        };
        if !(region.2 > 0.0 && region.3 > 0.0) {
            return None;
        }
        // Las primitivas: las del primer elemento de la cadena que tenga alguna.
        let (owner, prims) = chain.iter().find_map(|&c| {
            let kids: Vec<usize> = self
                .node(c)
                .children
                .iter()
                .copied()
                .filter(|&k| PRIMITIVES.contains(&self.node(k).name.as_str()))
                .collect();
            (!kids.is_empty()).then_some((c, kids))
        })?;
        // Escala de `primitiveUnits="objectBoundingBox"` para magnitudes (`stdDeviation`,
        // `dx`, …): fracciones de la caja.
        let (kx, ky) = match (prim_obb, bbox) {
            (true, Some((_, _, w, h))) => (w, h),
            _ => (1.0, 1.0),
        };
        let mut out: Vec<FilterPrimitive> = Vec::new();
        let mut results: HashMap<String, usize> = HashMap::new();
        for p in prims {
            let pn = self.node(p);
            let k = out.len();
            // `in` (§9.2): una referencia a un resultado que no existe cuenta como si no
            // hubiera `in`: el resultado anterior, o `SourceGraphic` en la primera.
            let input_of = |s: Option<&str>| -> FilterInput {
                match s.map(str::trim) {
                    Some("SourceGraphic") => FilterInput::SourceGraphic,
                    Some("SourceAlpha") => FilterInput::SourceAlpha,
                    Some("BackgroundImage" | "BackgroundAlpha" | "FillPaint" | "StrokePaint") => {
                        FilterInput::Transparent
                    }
                    Some(name) if results.contains_key(name) => {
                        FilterInput::Primitive(results[name])
                    }
                    _ if k == 0 => FilterInput::SourceGraphic,
                    _ => FilterInput::Primitive(k - 1),
                }
            };
            let input = input_of(pn.attr("in"));
            let in2 = input_of(pn.attr("in2"));
            let flood = |node: &DocNode, default_opacity: f32| -> Rgba {
                let st = self.computed(node, &fst, vp);
                let mut c = node
                    .props
                    .get("flood-color")
                    .and_then(|v| one(v))
                    .and_then(parse_color)
                    .map_or(Rgba::BLACK, |c| c.resolve(st.color));
                let o = node
                    .props
                    .get("flood-opacity")
                    .and_then(|v| cv_fraction(v))
                    .unwrap_or(default_opacity)
                    .clamp(0.0, 1.0);
                c.a *= o;
                c
            };
            let std_dev = |node: &DocNode, default: f32| -> (f32, f32) {
                // Un valor negativo desactiva la primitiva (§9.14); cero en un eje, sin
                // desenfoque en ese eje.
                let (sx, sy) = match node.attr("stdDeviation").and_then(geom::parse_numbers) {
                    Some(v) => match v.as_slice() {
                        [s] => (*s, *s),
                        [a, b] => (*a, *b),
                        _ => (0.0, 0.0),
                    },
                    None => (default, default),
                };
                if sx < 0.0 || sy < 0.0 || !sx.is_finite() || !sy.is_finite() {
                    (0.0, 0.0)
                } else {
                    (sx * kx, sy * ky)
                }
            };
            let (kind, inputs) = match pn.name.as_str() {
                "feGaussianBlur" => {
                    let edge = match pn.attr("edgeMode").map(str::trim) {
                        Some("duplicate") => EdgeMode::Duplicate,
                        Some("wrap") => EdgeMode::Wrap,
                        _ => EdgeMode::None,
                    };
                    let kind = FilterKind::GaussianBlur {
                        std_dev: std_dev(pn, 0.0),
                        edge,
                    };
                    (kind, vec![input])
                }
                "feOffset" => {
                    let dx = number(pn, "dx").unwrap_or(0.0) * kx;
                    let dy = number(pn, "dy").unwrap_or(0.0) * ky;
                    (FilterKind::Offset { dx, dy }, vec![input])
                }
                "feFlood" => (
                    FilterKind::Flood {
                        color: flood(pn, 1.0),
                    },
                    Vec::new(),
                ),
                "feMerge" => {
                    let mut inputs = Vec::new();
                    for &m in &pn.children {
                        let mn = self.node(m);
                        if mn.name == "feMergeNode" {
                            inputs.push(input_of(mn.attr("in")));
                        }
                    }
                    (
                        FilterKind::Merge {
                            inputs: inputs.clone(),
                        },
                        inputs,
                    )
                }
                "feColorMatrix" => (
                    FilterKind::ColorMatrix {
                        matrix: color_matrix(pn),
                    },
                    vec![input],
                ),
                "feComposite" => {
                    let op = match pn.attr("operator").map(str::trim) {
                        Some("in") => CompositeOp::In,
                        Some("out") => CompositeOp::Out,
                        Some("atop") => CompositeOp::Atop,
                        Some("xor") => CompositeOp::Xor,
                        Some("lighter") => CompositeOp::Lighter,
                        Some("arithmetic") => CompositeOp::Arithmetic {
                            k1: number(pn, "k1").unwrap_or(0.0),
                            k2: number(pn, "k2").unwrap_or(0.0),
                            k3: number(pn, "k3").unwrap_or(0.0),
                            k4: number(pn, "k4").unwrap_or(0.0),
                        },
                        _ => CompositeOp::Over,
                    };
                    (FilterKind::Composite { op, in2 }, vec![input, in2])
                }
                "feBlend" => {
                    let mode = match pn.attr("mode").map(str::trim) {
                        Some("multiply") => BlendMode::Multiply,
                        Some("screen") => BlendMode::Screen,
                        Some("darken") => BlendMode::Darken,
                        Some("lighten") => BlendMode::Lighten,
                        _ => BlendMode::Normal,
                    };
                    (FilterKind::Blend { mode, in2 }, vec![input, in2])
                }
                "feDropShadow" => {
                    let kind = FilterKind::DropShadow {
                        std_dev: std_dev(pn, 2.0),
                        dx: number(pn, "dx").unwrap_or(2.0) * kx,
                        dy: number(pn, "dy").unwrap_or(2.0) * ky,
                        color: flood(pn, 1.0),
                    };
                    (kind, vec![input])
                }
                // Sin soporte: deja pasar su entrada (mejor que borrar el elemento).
                _ => (FilterKind::Passthrough, vec![input]),
            };
            // Subregión (§9.4): por defecto, la unión de las de sus entradas; con una entrada
            // estándar o sin entradas, la región del filtro.
            let default = inputs
                .iter()
                .map(|i| match i {
                    FilterInput::Primitive(j) => Some(out[*j].subregion),
                    _ => None,
                })
                .try_fold(None, |acc: Option<Bbox>, s| {
                    s.map(|s| Some(acc.map_or(s, |a| union(a, s))))
                })
                .flatten()
                .unwrap_or(region);
            let sub = |name: &str, axis: Axis, def: f32, is_pos: bool| -> f32 {
                let a = pn.attr(name);
                if prim_obb {
                    let Some((bx, by, bw, bh)) = bbox else {
                        return def;
                    };
                    if a.is_none() {
                        return def;
                    }
                    let frac = Self::bbox_fraction(a, 0.0);
                    match (axis, is_pos) {
                        (Axis::X, true) => bx + frac * bw,
                        (Axis::X, false) => frac * bw,
                        (_, true) => by + frac * bh,
                        (_, false) => frac * bh,
                    }
                } else {
                    user_len(a, axis).unwrap_or(def)
                }
            };
            let subregion = (
                sub("x", Axis::X, default.0, true),
                sub("y", Axis::Y, default.1, true),
                sub("width", Axis::X, default.2, false),
                sub("height", Axis::Y, default.3, false),
            );
            let linear_rgb = self.linear_rgb(p, owner);
            if let Some(r) = pn.attr("result").map(str::trim).filter(|r| !r.is_empty()) {
                results.insert(r.to_owned(), k);
            }
            out.push(FilterPrimitive {
                kind,
                input,
                subregion,
                linear_rgb,
            });
        }
        Some(Filter {
            region,
            primitives: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{FilterInput, FilterKind, Node, Rgba, Svg, Tree};

    const NS: &str = "xmlns='http://www.w3.org/2000/svg'";

    fn tree(body: &str) -> Tree {
        let src = format!("<svg {NS} width='100' height='100'>{body}</svg>");
        Svg::parse(&src).expect("SVG válido").tree(100.0, 100.0)
    }

    fn filter_of(t: &Tree) -> &crate::Filter {
        match &t.root.children[0] {
            Node::Group(g) => g.filter.as_deref().expect("con filtro"),
            other => panic!("se esperaba un grupo: {other:?}"),
        }
    }

    #[test]
    fn region_por_defecto_y_flood() {
        let t = tree(
            "<filter id='f'><feFlood flood-color='lime' flood-opacity='0.5'/></filter>\
             <rect x='10' y='10' width='50' height='50' filter='url(#f)'/>",
        );
        let f = filter_of(&t);
        // objectBoundingBox: -10 %, -10 %, 120 %, 120 % de la caja.
        let (x, y, w, h) = f.region;
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(
            near(x, 5.0) && near(y, 5.0) && near(w, 60.0) && near(h, 60.0),
            "{:?}",
            f.region
        );
        assert_eq!(f.primitives.len(), 1);
        let p = &f.primitives[0];
        let mut lime = Rgba::rgb(0, 255, 0);
        lime.a = 0.5;
        assert_eq!(p.kind, FilterKind::Flood { color: lime });
        assert_eq!(p.subregion, f.region);
        assert!(p.linear_rgb);
    }

    #[test]
    fn referencia_inexistente_se_ignora_y_filtro_vacio_no_pinta() {
        // §5: sin filtro que aplicar, el elemento se pinta tal cual.
        let t = tree("<rect width='50' height='50' filter='url(#nada)'/>");
        assert!(matches!(t.root.children[0], Node::Path(_)));
        let t = tree("<rect width='50' height='50' filter='none'/>");
        assert!(matches!(t.root.children[0], Node::Path(_)));
        // §9.3: un `filter` sin primitivas deja el elemento sin pintar.
        let t = tree("<filter id='f'/><rect width='50' height='50' filter='url(#f)'/>");
        assert!(t.root.children.is_empty());
        // Región de ancho cero: tampoco se pinta.
        let t = tree(
            "<filter id='f' width='0'><feFlood/></filter>\
             <rect width='50' height='50' filter='url(#f)'/>",
        );
        assert!(t.root.children.is_empty());
    }

    #[test]
    fn unidades_entradas_y_resultados() {
        let t = tree(
            "<filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100' \
             primitiveUnits='objectBoundingBox' style='color-interpolation-filters: sRGB'>\
             <feGaussianBlur in='SourceAlpha' stdDeviation='0.1 0.05' result='b'/>\
             <feOffset dx='0.1' result='o' in='b'/>\
             <feMerge><feMergeNode in='o'/><feMergeNode in='SourceGraphic'/></feMerge>\
             <feColorMatrix in='nada' type='saturate' values='0'/>\
             </filter>\
             <rect x='20' y='20' width='40' height='20' filter='url(#f)'/>",
        );
        let f = filter_of(&t);
        assert_eq!(f.region, (0.0, 0.0, 100.0, 100.0));
        let p = &f.primitives;
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].input, FilterInput::SourceAlpha);
        match p[0].kind {
            FilterKind::GaussianBlur { std_dev, .. } => assert_eq!(std_dev, (4.0, 1.0)),
            ref k => panic!("{k:?}"),
        }
        assert_eq!(p[1].input, FilterInput::Primitive(0));
        assert_eq!(p[1].kind, FilterKind::Offset { dx: 4.0, dy: 0.0 });
        // La subregión por defecto es la de su entrada.
        assert_eq!(p[1].subregion, p[0].subregion);
        assert_eq!(
            p[2].kind,
            FilterKind::Merge {
                inputs: vec![FilterInput::Primitive(1), FilterInput::SourceGraphic]
            }
        );
        // Un `in` a un resultado inexistente toma el resultado anterior.
        assert_eq!(p[3].input, FilterInput::Primitive(2));
        assert!(p.iter().all(|p| !p.linear_rgb));
    }

    #[test]
    fn subregion_explicita_y_href() {
        let t = tree(
            "<filter id='base' filterUnits='userSpaceOnUse' x='0' y='0' width='80' height='80'>\
             <feFlood x='10' y='10' width='20' height='30'/></filter>\
             <filter id='f' href='#base'/>\
             <circle cx='50' cy='50' r='10' filter='url(#f)'/>",
        );
        let f = filter_of(&t);
        assert_eq!(f.region, (0.0, 0.0, 80.0, 80.0));
        assert_eq!(f.primitives[0].subregion, (10.0, 10.0, 20.0, 30.0));
    }
}
