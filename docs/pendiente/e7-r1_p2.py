import re
ROOT = 'D:/Personales/Repos/browser-freedom-army-e7/crates/bf-svg/src/'


def load(f):
    return open(ROOT + f, encoding='utf-8').read()


def save(f, s):
    open(ROOT + f, 'w', encoding='utf-8', newline='\n').write(s)


def rep(s, a, b, cnt=1):
    assert s.count(a) == cnt, (a[:80], s.count(a))
    return s.replace(a, b)


# ---------- lib.rs: tipos (preparado antes) ----------
lib = open('D:/Personales/Repos/browser-freedom-army-e7/target/e7/lib.rs.p2', encoding='utf-8').read()
save('lib.rs', lib)

# ---------- style.rs: nuevas propiedades de presentación ----------
s = load('style.rs')
s = rep(s, '''    "marker-start",
    "opacity",''', '''    "marker-start",
    "mask",
    "mask-type",
    "opacity",
    "paint-order",''')
s = rep(s, '''    "text-anchor",
    "visibility",
];''', '''    "text-anchor",
    "vector-effect",
    "visibility",
];''')
save('style.rs', s)

# ---------- geom.rs: longitud de un trazado ----------
s = load('geom.rs')
s = rep(s, '''pub fn bbox(segs: &[Seg]) -> Option<(f32, f32, f32, f32)> {''', '''/// Longitud de un trazado (para `pathLength`, SVG 2 §9.5.1): las curvas se aproximan con
/// 64 cuerdas.
pub fn length(segs: &[Seg]) -> f32 {
    const STEPS: usize = 64;
    let mut total = 0.0f32;
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    let dist = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    for s in segs {
        match *s {
            Seg::MoveTo(x, y) => {
                (cx, cy) = (x, y);
                (sx, sy) = (x, y);
            }
            Seg::LineTo(x, y) => {
                total += dist((cx, cy), (x, y));
                (cx, cy) = (x, y);
            }
            Seg::QuadTo(x1, y1, x, y) => {
                let mut prev = (cx, cy);
                for k in 1..=STEPS {
                    let t = k as f32 / STEPS as f32;
                    let u = 1.0 - t;
                    let p = (
                        u * u * cx + 2.0 * u * t * x1 + t * t * x,
                        u * u * cy + 2.0 * u * t * y1 + t * t * y,
                    );
                    total += dist(prev, p);
                    prev = p;
                }
                (cx, cy) = (x, y);
            }
            Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                let mut prev = (cx, cy);
                for k in 1..=STEPS {
                    let t = k as f32 / STEPS as f32;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    let p = (
                        a * cx + b * x1 + c * x2 + d * x,
                        a * cy + b * y1 + c * y2 + d * y,
                    );
                    total += dist(prev, p);
                    prev = p;
                }
                (cx, cy) = (x, y);
            }
            Seg::Close => {
                total += dist((cx, cy), (sx, sy));
                (cx, cy) = (sx, sy);
            }
        }
    }
    total
}

pub fn bbox(segs: &[Seg]) -> Option<(f32, f32, f32, f32)> {''')
s = rep(s, '''    fn arc_bbox() {''', '''    fn path_length() {
        let square = parse_path("M10,10L110,10L110,110L10,110Z");
        assert!((length(&square) - 400.0).abs() < 1e-3);
        let circle = ellipse(0.0, 0.0, 100.0, 100.0);
        let c = 2.0 * std::f32::consts::PI * 100.0;
        assert!((length(&circle) - c).abs() < 0.5, "{}", length(&circle));
    }

    #[test]
    fn arc_bbox() {''')
save('geom.rs', s)

# ---------- build.rs ----------
s = load('build.rs')
s = rep(s, '''    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, Intrinsic, LineCap, LineJoin,
    Node, Paint, Path, Rgba, Span, Spread, Stroke, Svg, Text, TextAnchor, Tree,
};''', '''    Chunk, ClipPath, Fill, FillRule, Gradient, GradientKind, Group, Intrinsic, LineCap, LineJoin,
    Mask, Node, Paint, Path, Pattern, Rgba, Span, Spread, Stroke, Svg, Text, TextAnchor, Tree,
};''')
# Literales de Group: añadir `mask: None` detrás de `clip`.
s, n = re.subn(r'\n(\s*)clip(: None)?,\n(\s*)children',
               lambda m: '\n%sclip%s,\n%smask: None,\n%schildren' % (m.group(1), m.group(2) or '', m.group(1), m.group(3)), s)
print('group literals', n)

s = rep(s, '''    /// `marker-start`, `marker-mid` y `marker-end` (URLs).
    markers: [Option<String>; 3],
}''', '''    /// `marker-start`, `marker-mid` y `marker-end` (URLs).
    markers: [Option<String>; 3],
    /// `paint-order` (SVG 2 §13.7.3): 0 relleno, 1 trazo, 2 marcadores.
    paint_order: [u8; 3],
}''')
s = rep(s, '''            markers: [None, None, None],
        }''', '''            markers: [None, None, None],
            paint_order: [0, 1, 2],
        }''')
s = rep(s, '''                "letter-spacing" => {''', '''                "paint-order" => {
                    let parts = toks(v);
                    let words: Vec<String> = parts
                        .iter()
                        .filter_map(|c| match c.token()? {
                            TokenKind::Ident(s) => Some(s.to_ascii_lowercase()),
                            _ => None,
                        })
                        .collect();
                    if words.len() != parts.len() || words.is_empty() || words.len() > 3 {
                        continue;
                    }
                    if words.len() == 1 && words[0] == "normal" {
                        st.paint_order = [0, 1, 2];
                        continue;
                    }
                    let mut order: Vec<u8> = Vec::new();
                    for w in &words {
                        let k = match w.as_str() {
                            "fill" => 0,
                            "stroke" => 1,
                            "markers" => 2,
                            _ => break,
                        };
                        if order.contains(&k) {
                            break;
                        }
                        order.push(k);
                    }
                    if order.len() != words.len() {
                        continue;
                    }
                    // Los que falten van detrás, en el orden por defecto.
                    for k in 0..3 {
                        if !order.contains(&k) {
                            order.push(k);
                        }
                    }
                    st.paint_order = [order[0], order[1], order[2]];
                }
                "letter-spacing" => {''')

# element(): máscara
s = rep(s, '''            .filter(|&c| self.node(c).name == "clipPath")
            .map(|c| (c, st.clone()));
        if opacity >= 1.0 && clip.is_none() {''', '''            .filter(|&c| self.node(c).name == "clipPath")
            .map(|c| (c, st.clone()));
        let mask = n
            .props
            .get("mask")
            .and_then(|v| one(v).and_then(url_of))
            .and_then(|u| self.by_id(&u))
            .filter(|&m| self.node(m).name == "mask");
        if opacity >= 1.0 && clip.is_none() && mask.is_none() {''')
s = rep(s, '''            None => None,
        };
        Some(Node::Group(Group {
            transform,
            opacity,
            clip,
            mask: None,
            children: vec![node],
        }))
    }''', '''            None => None,
        };
        let mask = match mask {
            Some(m) => {
                let bbox = node_bbox(&node, &Transform::IDENTITY);
                Some(Box::new(self.mask(m, vp, bbox)?))
            }
            None => None,
        };
        Some(Node::Group(Group {
            transform,
            opacity,
            clip,
            mask,
            children: vec![node],
        }))
    }

    /// Estilo base de un recurso (`marker`, `pattern`, `mask`): sus propiedades heredan de sus
    /// ancestros, no del elemento que lo usa; aproximamos con lo heredado del documento.
    fn resource_st(&self) -> St {
        St {
            color: self.svg.inherited.color,
            size: self.svg.inherited.font_size,
            family: self.svg.inherited.font_family.clone(),
            ..St::default()
        }
    }

    /// Un `x`/`y`/`width`/`height` de un recurso en `objectBoundingBox`: fracción (o
    /// porcentaje) de la caja.
    fn bbox_fraction(a: Option<&str>, default: f32) -> f32 {
        match a.and_then(parse_len) {
            Some(Len::Pct(p)) => p / 100.0,
            Some(Len::Px(v)) => v,
            _ => default,
        }
    }

    /// `mask` referenciada por un elemento con caja `bbox` (CSS Masking 1 §7.1). Si la región
    /// queda vacía, una máscara vacía: el elemento no se ve.
    fn mask(&mut self, m: usize, vp: Viewport, bbox: Option<Bbox>) -> Option<Mask> {
        let empty = || Mask {
            luminance: true,
            rect: (0.0, 0.0, 0.0, 0.0),
            content: Group::default(),
        };
        if self.stack.contains(&m) {
            return Some(empty());
        }
        let mn = self.node(m);
        let user_units = mn
            .attr("maskUnits")
            .is_some_and(|u| u.trim() == "userSpaceOnUse");
        let content_bbox = mn
            .attr("maskContentUnits")
            .is_some_and(|u| u.trim() == "objectBoundingBox");
        let mst = self.computed(mn, &self.resource_st(), vp);
        let rect = if user_units {
            let get = |name: &str, def: &str, axis: Axis| {
                self.len(mn, name, axis, vp, &mst)
                    .or_else(|| parse_len(def).map(|l| vp.resolve(l, axis, mst.size)))
                    .unwrap_or(0.0)
            };
            (
                get("x", "-10%", Axis::X),
                get("y", "-10%", Axis::Y),
                get("width", "120%", Axis::X),
                get("height", "120%", Axis::Y),
            )
        } else {
            let Some((bx, by, bw, bh)) = bbox.filter(|b| b.2 > 0.0 && b.3 > 0.0) else {
                return Some(empty());
            };
            (
                bx + Self::bbox_fraction(mn.attr("x"), -0.1) * bw,
                by + Self::bbox_fraction(mn.attr("y"), -0.1) * bh,
                Self::bbox_fraction(mn.attr("width"), 1.2) * bw,
                Self::bbox_fraction(mn.attr("height"), 1.2) * bh,
            )
        };
        if rect.2 <= 0.0 || rect.3 <= 0.0 {
            return Some(empty());
        }
        let content_t = match (content_bbox, bbox) {
            (true, Some((bx, by, bw, bh))) => {
                Transform::translate(bx, by).then(&Transform::scale(bw, bh))
            }
            (true, None) => return Some(empty()),
            (false, _) => Transform::IDENTITY,
        };
        let luminance = mn
            .props
            .get("mask-type")
            .and_then(|v| ident(v))
            .is_none_or(|t| t != "alpha");
        self.stack.push(m);
        let children = self.children(m, &mst, vp);
        self.stack.pop();
        Some(Mask {
            luminance,
            rect,
            content: Group {
                transform: content_t,
                opacity: 1.0,
                clip: None,
                mask: None,
                children,
            },
        })
    }''')

# content(): pathLength, vector-effect, paint-order
s = rep(s, '''        let path = Node::Path(Path {
            fill: self.fill(st, bbox, vp),
            stroke: self.stroke(st, bbox, vp),
            segs,
            transform: Transform::IDENTITY,
        });
        if markers.is_empty() {
            return Some(path);
        }
        let mut children = vec![path];
        children.extend(markers);
        Some(Node::Group(Group {
            opacity: 1.0,
            children,
            ..Group::default()
        }))
    }''', '''        let n = self.node(i);
        let fill = self.fill(st, bbox, vp);
        let mut stroke = self.stroke(st, bbox, vp);
        if let Some(s) = &mut stroke {
            // `pathLength` (§9.5.1): las distancias del trazo se escalan por longitud real /
            // `pathLength`; un valor negativo es un error (se ignora) y cero escala a infinito.
            if let Some(pl) = n
                .attr("pathLength")
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| *v >= 0.0 && v.is_finite())
                && !s.dasharray.is_empty()
            {
                if pl == 0.0 {
                    if s.dasharray[0] > 0.0 {
                        s.dasharray.clear();
                    } else {
                        for d in &mut s.dasharray {
                            *d = if *d > 0.0 { 1e9 } else { 0.0 };
                        }
                        s.dashoffset = 0.0;
                    }
                } else {
                    let k = geom::length(&segs) / pl;
                    for d in &mut s.dasharray {
                        *d *= k;
                    }
                    s.dashoffset *= k;
                }
            }
            s.non_scaling = n
                .props
                .get("vector-effect")
                .and_then(|v| ident(v))
                .is_some_and(|v| v == "non-scaling-stroke");
        }
        let default_order = st.paint_order == [0, 1, 2];
        if markers.is_empty() && (default_order || fill.is_none() || stroke.is_none()) {
            return Some(Node::Path(Path {
                fill,
                stroke,
                segs,
                transform: Transform::IDENTITY,
            }));
        }
        // `paint-order`: relleno, trazo y marcadores como nodos aparte, en su orden.
        let (mut fill, mut stroke, mut markers) = (fill, stroke, Some(markers));
        let mut children = Vec::new();
        for k in st.paint_order {
            match k {
                0 => {
                    if let Some(f) = fill.take() {
                        children.push(Node::Path(Path {
                            fill: Some(f),
                            stroke: None,
                            segs: segs.clone(),
                            transform: Transform::IDENTITY,
                        }));
                    }
                }
                1 => {
                    if let Some(s) = stroke.take() {
                        children.push(Node::Path(Path {
                            fill: None,
                            stroke: Some(s),
                            segs: segs.clone(),
                            transform: Transform::IDENTITY,
                        }));
                    }
                }
                _ => children.extend(markers.take().unwrap_or_default()),
            }
        }
        Some(Node::Group(Group {
            opacity: 1.0,
            children,
            ..Group::default()
        }))
    }''')

# marker(): estilo base de recurso
s = rep(s, '''        let mn = self.node(m);
        let base = St {
            color: self.svg.inherited.color,
            size: self.svg.inherited.font_size,
            family: self.svg.inherited.font_family.clone(),
            ..St::default()
        };
        let mst = self.computed(mn, &base, vp);''', '''        let mn = self.node(m);
        let mst = self.computed(mn, &self.resource_st(), vp);''')

# resolve_paint / fill / stroke -> &mut self, con patrones
s = rep(s, '''    fn resolve_paint(
        &self,
        spec: &PaintSpec,
        st: &St,
        bbox: Option<Bbox>,
        vp: Viewport,
    ) -> Option<Paint> {
        match spec {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(Paint::Color(*c)),
            PaintSpec::CurrentColor => Some(Paint::Color(st.color)),
            PaintSpec::Url(url, fallback) => {
                let server = self.by_id(url).filter(|&s| {
                    matches!(
                        self.node(s).name.as_str(),
                        "linearGradient" | "radialGradient"
                    )
                });
                match server.map(|s| self.gradient(s, bbox, vp, st)) {''', '''    fn resolve_paint(
        &mut self,
        spec: &PaintSpec,
        st: &St,
        bbox: Option<Bbox>,
        vp: Viewport,
    ) -> Option<Paint> {
        match spec {
            PaintSpec::None => None,
            PaintSpec::Color(c) => Some(Paint::Color(*c)),
            PaintSpec::CurrentColor => Some(Paint::Color(st.color)),
            PaintSpec::Url(url, fallback) => {
                let server = self.by_id(url).filter(|&s| {
                    matches!(
                        self.node(s).name.as_str(),
                        "linearGradient" | "radialGradient" | "pattern"
                    )
                });
                let result = match server {
                    Some(s) if self.node(s).name == "pattern" => Some(self.pattern(s, bbox, vp)),
                    Some(s) => Some(self.gradient(s, bbox, vp, st)),
                    None => None,
                };
                match result {''')
s = rep(s, '''    fn fill(&self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Fill> {''',
        '''    fn fill(&mut self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Fill> {''')
s = rep(s, '''    fn stroke(&self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Stroke> {''',
        '''    fn stroke(&mut self, st: &St, bbox: Option<Bbox>, vp: Viewport) -> Option<Stroke> {''')
s = rep(s, '''            dasharray: dash,
            dashoffset: st.dash_offset,
        })
    }''', '''            dasharray: dash,
            dashoffset: st.dash_offset,
            non_scaling: false,
        })
    }

    /// Cadena de `href` de un `pattern` (él primero, SVG 2 §14.4.1).
    fn pattern_chain(&self, s: usize) -> Vec<usize> {
        let mut out = vec![s];
        while out.len() < MAX_DEPTH {
            let last = self.node(out[out.len() - 1]);
            match last.attr("href").and_then(|h| self.by_id(h)) {
                Some(next) if !out.contains(&next) && self.node(next).name == "pattern" => {
                    out.push(next)
                }
                _ => break,
            }
        }
        out
    }

    /// `pattern` (SVG 2 §14.4): `Err` si es una referencia inválida (transformación no
    /// invertible o ciclo), `Ok(None)` si no pinta nada (tesela vacía o `viewBox` nulo).
    fn pattern(&mut self, s: usize, bbox: Option<Bbox>, vp: Viewport) -> Result<Option<Paint>, ()> {
        if self.stack.contains(&s) {
            return Err(());
        }
        let chain = self.pattern_chain(s);
        let attr = |name: &str| chain.iter().find_map(|&c| self.node(c).attr(name));
        let pt = attr("patternTransform")
            .and_then(geom::parse_transform)
            .unwrap_or_default();
        if !pt.is_invertible() {
            return Err(());
        }
        let bbox_units = attr("patternUnits").is_none_or(|u| u.trim() != "userSpaceOnUse");
        let content_bbox =
            attr("patternContentUnits").is_some_and(|u| u.trim() == "objectBoundingBox");
        let pst = self.computed(self.node(s), &self.resource_st(), vp);
        let (x, y, w, h) = if bbox_units {
            let Some((bx, by, bw, bh)) = bbox.filter(|b| b.2 > 0.0 && b.3 > 0.0) else {
                return Ok(None);
            };
            (
                bx + Self::bbox_fraction(attr("x"), 0.0) * bw,
                by + Self::bbox_fraction(attr("y"), 0.0) * bh,
                Self::bbox_fraction(attr("width"), 0.0) * bw,
                Self::bbox_fraction(attr("height"), 0.0) * bh,
            )
        } else {
            let get = |name: &str, axis: Axis| {
                attr(name)
                    .and_then(parse_len)
                    .map_or(0.0, |l| vp.resolve(l, axis, pst.size))
            };
            (
                get("x", Axis::X),
                get("y", Axis::Y),
                get("width", Axis::X),
                get("height", Axis::Y),
            )
        };
        if w <= 0.0 || h <= 0.0 {
            return Ok(None);
        }
        let mut view_box = None;
        for &c in &chain {
            match view_box_state(self.node(c)) {
                Err(()) => return Ok(None),
                Ok(Some(vb)) => {
                    view_box = Some(vb);
                    break;
                }
                Ok(None) => {}
            }
        }
        let content_t = match (view_box, content_bbox, bbox) {
            (Some(vb), _, _) => {
                view_box_transform(vb, attr("preserveAspectRatio").unwrap_or(""), (w, h))
            }
            (None, true, Some((_, _, bw, bh))) => Transform::scale(bw, bh),
            (None, true, None) => return Ok(None),
            (None, false, _) => Transform::IDENTITY,
        };
        let inner_vp = match view_box {
            Some((_, _, vw, vh)) => Viewport { w: vw, h: vh },
            None => vp,
        };
        // El contenido es el del primero de la cadena que tenga hijos elemento.
        let content = chain
            .iter()
            .copied()
            .find(|&c| {
                self.node(c)
                    .children
                    .iter()
                    .any(|&k| !self.node(k).is_text())
            })
            .unwrap_or(s);
        self.stack.push(s);
        let children = self.children(content, &pst, inner_vp);
        self.stack.pop();
        Ok(Some(Paint::Pattern(Box::new(Pattern {
            width: w,
            height: h,
            transform: pt.then(&Transform::translate(x, y)),
            content: Group {
                transform: content_t,
                opacity: 1.0,
                clip: None,
                mask: None,
                children,
            },
        }))))
    }''')
# Span: stroke_first
s = rep(s, '''                    dx: std::mem::take(&mut dx),
                    dy: std::mem::take(&mut dy),
                };''', '''                    dx: std::mem::take(&mut dx),
                    dy: std::mem::take(&mut dy),
                    stroke_first: st.paint_order.iter().position(|&k| k == 1)
                        < st.paint_order.iter().position(|&k| k == 0),
                };''')
save('build.rs', s)
print('ok')
