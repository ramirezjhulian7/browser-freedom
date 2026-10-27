//! Geometría de SVG 2: transformaciones (§8.5 `transform`), datos de trazado (§9.3, con los
//! arcos de la implementación §B.2.4 convertidos a cúbicas), listas de puntos y cajas
//! envolventes.

use std::f32::consts::PI;

/// Matriz afín `[a b c d e f]`: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f32, y: f32) -> Transform {
        Transform {
            e: x,
            f: y,
            ..Transform::IDENTITY
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Transform {
        Transform {
            a: sx,
            d: sy,
            ..Transform::IDENTITY
        }
    }

    pub fn rotate(deg: f32) -> Transform {
        let (s, c) = (deg * PI / 180.0).sin_cos();
        Transform {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self · other`: primero `other`, después `self`.
    pub fn then(&self, other: &Transform) -> Transform {
        let (m, n) = (self, other);
        Transform {
            a: m.a * n.a + m.c * n.b,
            b: m.b * n.a + m.d * n.b,
            c: m.a * n.c + m.c * n.d,
            d: m.b * n.c + m.d * n.d,
            e: m.a * n.e + m.c * n.f + m.e,
            f: m.b * n.e + m.d * n.f + m.f,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub fn is_identity(&self) -> bool {
        *self == Transform::IDENTITY
    }

    /// ¿Es invertible (no aplasta el plano)?
    pub fn is_invertible(&self) -> bool {
        let det = self.a * self.d - self.b * self.c;
        det.is_finite() && det.abs() > 1e-12
    }

    /// Escala media (para pasar longitudes de usuario a píxeles).
    pub fn mean_scale(&self) -> f32 {
        ((self.a * self.d - self.b * self.c).abs()).sqrt()
    }
}

/// Un segmento de trazado en coordenadas absolutas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Lector de números de las microsintaxis de SVG (`d`, `points`, `viewBox`, `transform`):
/// separados por espacios y/o una coma, con signos y exponentes pegados (`1-2`, `.5.5`).
pub(crate) struct Numbers<'a> {
    s: &'a [u8],
    pub pos: usize,
}

impl<'a> Numbers<'a> {
    pub fn new(s: &'a str) -> Self {
        Numbers {
            s: s.as_bytes(),
            pos: 0,
        }
    }

    pub fn skip_ws(&mut self) {
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Salta espacios y a lo sumo una coma.
    pub fn skip_sep(&mut self) {
        self.skip_ws();
        if self.peek() == Some(b',') {
            self.pos += 1;
            self.skip_ws();
        }
    }

    pub fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    pub fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.pos >= self.s.len()
    }

    pub fn number(&mut self) -> Option<f32> {
        self.skip_ws();
        let start = self.pos;
        let mut i = self.pos;
        if matches!(self.s.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let digits = |i: &mut usize| {
            let s = *i;
            while self.s.get(*i).is_some_and(u8::is_ascii_digit) {
                *i += 1;
            }
            *i > s
        };
        let int = digits(&mut i);
        let mut frac = false;
        if self.s.get(i) == Some(&b'.') {
            i += 1;
            frac = digits(&mut i);
        }
        if !int && !frac {
            return None;
        }
        if matches!(self.s.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(self.s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if digits(&mut j) {
                i = j;
            }
        }
        let v: f32 = std::str::from_utf8(&self.s[start..i]).ok()?.parse().ok()?;
        self.pos = i;
        v.is_finite().then_some(v)
    }

    /// Una bandera de arco: `0` o `1`, que puede ir pegada a lo que sigue.
    pub fn flag(&mut self) -> Option<bool> {
        self.skip_ws();
        let v = match self.peek()? {
            b'0' => false,
            b'1' => true,
            _ => return None,
        };
        self.pos += 1;
        Some(v)
    }
}

/// Parsea los datos de un trazado (§9.3.9). Ante un error se queda con lo leído hasta ahí
/// (§9.5.4).
pub fn parse_path(d: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut n = Numbers::new(d);
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    // Último punto de control (para S/T), y si el comando anterior era cúbico o cuadrático.
    let mut last_ctrl: Option<(f32, f32, bool)> = None;
    let mut cmd = 0u8;
    loop {
        n.skip_ws();
        let Some(c) = n.peek() else {
            break;
        };
        if c.is_ascii_alphabetic() {
            cmd = c;
            n.pos += 1;
        } else if cmd == 0 || matches!(cmd, b'Z' | b'z') {
            break;
        } else if cmd == b'M' {
            cmd = b'L';
        } else if cmd == b'm' {
            cmd = b'l';
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
        let mut ok = true;
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            b'M' => match (n.number(), n.next_coord()) {
                (Some(x), Some(y)) => {
                    (cx, cy) = (ox + x, oy + y);
                    (sx, sy) = (cx, cy);
                    out.push(Seg::MoveTo(cx, cy));
                }
                _ => ok = false,
            },
            b'L' => match (n.number(), n.next_coord()) {
                (Some(x), Some(y)) => {
                    (cx, cy) = (ox + x, oy + y);
                    out.push(Seg::LineTo(cx, cy));
                }
                _ => ok = false,
            },
            b'H' => match n.number() {
                Some(x) => {
                    cx = if rel { cx + x } else { x };
                    out.push(Seg::LineTo(cx, cy));
                }
                None => ok = false,
            },
            b'V' => match n.number() {
                Some(y) => {
                    cy = if rel { cy + y } else { y };
                    out.push(Seg::LineTo(cx, cy));
                }
                None => ok = false,
            },
            b'C' => match n.coords::<6>() {
                Some([x1, y1, x2, y2, x, y]) => {
                    let (x1, y1, x2, y2) = (ox + x1, oy + y1, ox + x2, oy + y2);
                    (cx, cy) = (ox + x, oy + y);
                    out.push(Seg::CubicTo(x1, y1, x2, y2, cx, cy));
                    ctrl = Some((x2, y2, true));
                }
                None => ok = false,
            },
            b'S' => match n.coords::<4>() {
                Some([x2, y2, x, y]) => {
                    let (x1, y1) = match last_ctrl {
                        Some((px, py, true)) => (2.0 * cx - px, 2.0 * cy - py),
                        _ => (cx, cy),
                    };
                    let (x2, y2) = (ox + x2, oy + y2);
                    (cx, cy) = (ox + x, oy + y);
                    out.push(Seg::CubicTo(x1, y1, x2, y2, cx, cy));
                    ctrl = Some((x2, y2, true));
                }
                None => ok = false,
            },
            b'Q' => match n.coords::<4>() {
                Some([x1, y1, x, y]) => {
                    let (x1, y1) = (ox + x1, oy + y1);
                    (cx, cy) = (ox + x, oy + y);
                    out.push(Seg::QuadTo(x1, y1, cx, cy));
                    ctrl = Some((x1, y1, false));
                }
                None => ok = false,
            },
            b'T' => match n.coords::<2>() {
                Some([x, y]) => {
                    let (x1, y1) = match last_ctrl {
                        Some((px, py, false)) => (2.0 * cx - px, 2.0 * cy - py),
                        _ => (cx, cy),
                    };
                    (cx, cy) = (ox + x, oy + y);
                    out.push(Seg::QuadTo(x1, y1, cx, cy));
                    ctrl = Some((x1, y1, false));
                }
                None => ok = false,
            },
            b'A' => {
                let arc = (|| {
                    let rx = n.number()?;
                    n.skip_sep();
                    let ry = n.number()?;
                    n.skip_sep();
                    let rot = n.number()?;
                    n.skip_sep();
                    let large = n.flag()?;
                    n.skip_sep();
                    let sweep = n.flag()?;
                    n.skip_sep();
                    let x = n.number()?;
                    let y = n.next_coord()?;
                    Some((rx, ry, rot, large, sweep, x, y))
                })();
                match arc {
                    Some((rx, ry, rot, large, sweep, x, y)) => {
                        let (x, y) = (ox + x, oy + y);
                        arc_to(&mut out, (cx, cy), (rx, ry), rot, large, sweep, (x, y));
                        (cx, cy) = (x, y);
                    }
                    None => ok = false,
                }
            }
            b'Z' => {
                out.push(Seg::Close);
                (cx, cy) = (sx, sy);
            }
            _ => ok = false,
        }
        if !ok {
            break;
        }
        // Un trazado debe empezar con un moveto.
        if !matches!(out.first(), None | Some(Seg::MoveTo(..))) {
            out.clear();
            break;
        }
        last_ctrl = ctrl;
        n.skip_sep();
    }
    out
}

impl Numbers<'_> {
    /// El segundo número de un par (salta el separador).
    fn next_coord(&mut self) -> Option<f32> {
        self.skip_sep();
        self.number()
    }

    fn coords<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut out = [0.0; N];
        for (i, v) in out.iter_mut().enumerate() {
            if i > 0 {
                self.skip_sep();
            }
            *v = self.number()?;
        }
        Some(out)
    }
}

/// Arco elíptico de `from` a `to` como cúbicas (SVG 2 §B.2.4 y §B.2.5: radios fuera de rango
/// corregidos, radio cero = línea recta).
fn arc_to(
    out: &mut Vec<Seg>,
    (x1, y1): (f32, f32),
    (rx, ry): (f32, f32),
    rot: f32,
    large: bool,
    sweep: bool,
    (x2, y2): (f32, f32),
) {
    if (x1, y1) == (x2, y2) {
        return;
    }
    let (mut rx, mut ry) = (f64::from(rx).abs(), f64::from(ry).abs());
    if rx == 0.0 || ry == 0.0 {
        out.push(Seg::LineTo(x2, y2));
        return;
    }
    let (x1, y1, x2, y2) = (f64::from(x1), f64::from(y1), f64::from(x2), f64::from(y2));
    let phi = f64::from(rot).to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos * dx + sin * dy;
    let y1p = -sin * dx + cos * dy;
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den == 0.0 {
        0.0
    } else {
        (num / den).max(0.0).sqrt()
    };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
    let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= 2.0 * std::f64::consts::PI;
    } else if sweep && delta < 0.0 {
        delta += 2.0 * std::f64::consts::PI;
    }
    let segments = (delta.abs() / (std::f64::consts::FRAC_PI_2) - 1e-7)
        .ceil()
        .max(1.0) as usize;
    let step = delta / segments as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| {
        let (s, c) = t.sin_cos();
        (
            cx + rx * c * cos - ry * s * sin,
            cy + rx * c * sin + ry * s * cos,
        )
    };
    let deriv = |t: f64| {
        let (s, c) = t.sin_cos();
        (-rx * s * cos - ry * c * sin, -rx * s * sin + ry * c * cos)
    };
    let mut t = theta1;
    for i in 0..segments {
        let t2 = t + step;
        let (p1, p2) = (point(t), point(t2));
        let (d1, d2) = (deriv(t), deriv(t2));
        let end = if i + 1 == segments { (x2, y2) } else { p2 };
        out.push(Seg::CubicTo(
            (p1.0 + k * d1.0) as f32,
            (p1.1 + k * d1.1) as f32,
            (p2.0 - k * d2.0) as f32,
            (p2.1 - k * d2.1) as f32,
            end.0 as f32,
            end.1 as f32,
        ));
        t = t2;
    }
}

/// Lista de puntos de `polyline`/`polygon` (§10.7); un número suelto al final se ignora.
pub fn parse_points(s: &str) -> Vec<(f32, f32)> {
    let mut n = Numbers::new(s);
    let mut out = Vec::new();
    loop {
        n.skip_sep();
        let Some(x) = n.number() else {
            break;
        };
        n.skip_sep();
        let Some(y) = n.number() else {
            break;
        };
        out.push((x, y));
    }
    out
}

/// Lista de números (`viewBox`, `stroke-dasharray` sin unidades).
pub fn parse_numbers(s: &str) -> Option<Vec<f32>> {
    let mut n = Numbers::new(s);
    let mut out = Vec::new();
    while !n.at_end() {
        out.push(n.number()?);
        n.skip_sep();
    }
    Some(out)
}

/// Atributo `transform` (§8.5): lista de funciones, aplicadas de izquierda a derecha.
pub fn parse_transform(s: &str) -> Option<Transform> {
    let mut t = Transform::IDENTITY;
    let mut n = Numbers::new(s);
    loop {
        n.skip_sep();
        if n.at_end() {
            break;
        }
        let start = n.pos;
        while n.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            n.pos += 1;
        }
        let name = s.get(start..n.pos)?.to_owned();
        n.skip_ws();
        if n.peek() != Some(b'(') {
            return None;
        }
        n.pos += 1;
        let mut args = Vec::new();
        loop {
            n.skip_sep();
            if n.peek() == Some(b')') {
                n.pos += 1;
                break;
            }
            args.push(n.number()?);
        }
        let m = match (name.as_str(), args.as_slice()) {
            ("matrix", &[a, b, c, d, e, f]) => Transform { a, b, c, d, e, f },
            ("translate", &[x]) => Transform::translate(x, 0.0),
            ("translate", &[x, y]) => Transform::translate(x, y),
            ("scale", &[s]) => Transform::scale(s, s),
            ("scale", &[x, y]) => Transform::scale(x, y),
            ("rotate", &[a]) => Transform::rotate(a),
            ("rotate", &[a, x, y]) => Transform::translate(x, y)
                .then(&Transform::rotate(a))
                .then(&Transform::translate(-x, -y)),
            ("skewX", &[a]) => Transform {
                c: (a * PI / 180.0).tan(),
                ..Transform::IDENTITY
            },
            ("skewY", &[a]) => Transform {
                b: (a * PI / 180.0).tan(),
                ..Transform::IDENTITY
            },
            _ => return None,
        };
        t = t.then(&m);
    }
    Some(t)
}

/// Caja envolvente exacta de un trazado (extremos de las curvas incluidos): x, y, ancho, alto.
pub fn bbox(segs: &[Seg]) -> Option<(f32, f32, f32, f32)> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x: f32, y: f32| {
        b = Some(match b {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    };
    let (mut cx, mut cy) = (0.0, 0.0);
    let (mut sx, mut sy) = (0.0, 0.0);
    for s in segs {
        match *s {
            Seg::MoveTo(x, y) => {
                (cx, cy, sx, sy) = (x, y, x, y);
                add(x, y);
            }
            Seg::LineTo(x, y) => {
                (cx, cy) = (x, y);
                add(x, y);
            }
            Seg::QuadTo(x1, y1, x, y) => {
                // Elevada a cúbica.
                let c1 = (cx + 2.0 / 3.0 * (x1 - cx), cy + 2.0 / 3.0 * (y1 - cy));
                let c2 = (x + 2.0 / 3.0 * (x1 - x), y + 2.0 / 3.0 * (y1 - y));
                cubic_extrema((cx, cy), c1, c2, (x, y), &mut add);
                (cx, cy) = (x, y);
            }
            Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                cubic_extrema((cx, cy), (x1, y1), (x2, y2), (x, y), &mut add);
                (cx, cy) = (x, y);
            }
            Seg::Close => (cx, cy) = (sx, sy),
        }
    }
    b.map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0, y1 - y0))
}

fn cubic_extrema(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    add: &mut impl FnMut(f32, f32),
) {
    add(p3.0, p3.1);
    let at = |t: f32, a: f32, b: f32, c: f32, d: f32| {
        let u = 1.0 - t;
        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
    };
    let roots = |a: f32, b: f32, c: f32, d: f32| -> Vec<f32> {
        // Derivada: 3(-a + 3b - 3c + d)t² + 6(a - 2b + c)t + 3(b - a).
        let qa = -a + 3.0 * b - 3.0 * c + d;
        let qb = 2.0 * (a - 2.0 * b + c);
        let qc = b - a;
        let mut out = Vec::new();
        if qa.abs() < 1e-9 {
            if qb.abs() > 1e-9 {
                out.push(-qc / qb);
            }
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc >= 0.0 {
                let r = disc.sqrt();
                out.push((-qb + r) / (2.0 * qa));
                out.push((-qb - r) / (2.0 * qa));
            }
        }
        out.retain(|t| *t > 0.0 && *t < 1.0);
        out
    };
    let mut ts = roots(p0.0, p1.0, p2.0, p3.0);
    ts.extend(roots(p0.1, p1.1, p2.1, p3.1));
    for t in ts {
        add(at(t, p0.0, p1.0, p2.0, p3.0), at(t, p0.1, p1.1, p2.1, p3.1));
    }
}

/// Elipse (o círculo) como cuatro cúbicas, empezando en (cx + rx, cy) y en sentido horario
/// en pantalla (§10.4: el trazado empieza en el punto de ángulo 0).
pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<Seg> {
    let k = 0.552_284_8;
    let (ox, oy) = (rx * k, ry * k);
    vec![
        Seg::MoveTo(cx + rx, cy),
        Seg::CubicTo(cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry),
        Seg::CubicTo(cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy),
        Seg::CubicTo(cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry),
        Seg::CubicTo(cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy),
        Seg::Close,
    ]
}

/// Rectángulo con esquinas redondeadas (§10.2: el trazado empieza en (x + rx, y)).
pub fn rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) -> Vec<Seg> {
    if rx <= 0.0 || ry <= 0.0 {
        return vec![
            Seg::MoveTo(x, y),
            Seg::LineTo(x + w, y),
            Seg::LineTo(x + w, y + h),
            Seg::LineTo(x, y + h),
            Seg::Close,
        ];
    }
    let k = 0.552_284_8;
    let (ox, oy) = (rx * k, ry * k);
    let (r, b) = (x + w, y + h);
    vec![
        Seg::MoveTo(x + rx, y),
        Seg::LineTo(r - rx, y),
        Seg::CubicTo(r - rx + ox, y, r, y + ry - oy, r, y + ry),
        Seg::LineTo(r, b - ry),
        Seg::CubicTo(r, b - ry + oy, r - rx + ox, b, r - rx, b),
        Seg::LineTo(x + rx, b),
        Seg::CubicTo(x + rx - ox, b, x, b - ry + oy, x, b - ry),
        Seg::LineTo(x, y + ry),
        Seg::CubicTo(x, y + ry - oy, x + rx - ox, y, x + rx, y),
        Seg::Close,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_syntax() {
        assert_eq!(
            parse_path("M10-20l5.5.5zM0,0 1e1 2"),
            vec![
                Seg::MoveTo(10.0, -20.0),
                Seg::LineTo(15.5, -19.5),
                Seg::Close,
                Seg::MoveTo(0.0, 0.0),
                Seg::LineTo(10.0, 2.0),
            ]
        );
        // Error: se queda con lo leído.
        assert_eq!(parse_path("M1 2 L3"), vec![Seg::MoveTo(1.0, 2.0)]);
        assert_eq!(parse_path("L1 2"), vec![]);
        // Banderas de arco pegadas.
        let arc = parse_path("M0 0a5 5 0 1010 0");
        assert_eq!(arc.len(), 3);
        let Some(Seg::CubicTo(.., x, y)) = arc.last() else {
            panic!()
        };
        assert_eq!((*x, *y), (10.0, 0.0));
    }

    #[test]
    fn arc_bbox() {
        // Semicírculo de radio 5 por arriba (sweep 1 desde la izquierda: va por y < 0).
        let segs = parse_path("M0 0 A5 5 0 0 1 10 0");
        let (x, y, w, h) = bbox(&segs).unwrap();
        assert!((x - 0.0).abs() < 1e-3 && (w - 10.0).abs() < 1e-3);
        assert!((y + 5.0).abs() < 1e-2 && (h - 5.0).abs() < 1e-2, "{y} {h}");
    }

    #[test]
    fn transforms() {
        let t = parse_transform("translate(10,20) scale(2)").unwrap();
        assert_eq!(t.apply(1.0, 1.0), (12.0, 22.0));
        let r = parse_transform("rotate(90 10 10)").unwrap();
        let (x, y) = r.apply(20.0, 10.0);
        assert!((x - 10.0).abs() < 1e-4 && (y - 20.0).abs() < 1e-4);
        assert!(parse_transform("translate(1,").is_none());
        assert_eq!(parse_transform(""), Some(Transform::IDENTITY));
    }
}
