//! Iconos de la interfaz dibujados como paths (líneas de 1.5 px con extremos y uniones
//! redondeados) en una grilla de 16×16 escalada al rectángulo pedido. Nada de fuentes de
//! iconos. La marca es el logo vectorizado (relleno), y el indicador de carga de las pestañas
//! un arco que gira ([`spinner`]).

use bf_layout::fragment::Rect;
use bf_paint::{DisplayItem, PathOp};
use bf_style::values::color::Rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Back,
    Forward,
    Reload,
    Stop,
    Lock,
    /// El panel del motor: un medidor.
    Engine,
    /// El logo: el marco abierto y el pájaro que sale por la esquina (relleno).
    Brand,
    /// Página sin favicon (las pestañas).
    Globe,
    /// Conexión sin cifrar (`http:`): un círculo con un signo de exclamación.
    Insecure,
    /// El campo de URL con foco: una lupa.
    Search,
    /// Pestaña nueva.
    Plus,
    /// Cerrar una pestaña (la misma cruz que detener, más chica).
    Close,
}

/// El contorno del logo (`assets/browser-freedom.png` vectorizado) en la grilla de 16: el
/// marco con la esquina superior derecha abierta y el pájaro, en una sola figura.
#[rustfmt::skip]
const BRAND: &[(f32, f32)] = &[
    (16.00, 4.95), (15.70, 4.85), (15.04, 4.51), (14.77, 4.45), (14.21, 4.45), (13.88, 4.55),
    (13.51, 4.75), (13.18, 5.05), (12.81, 5.51), (12.65, 4.71), (12.41, 4.22), (12.08, 3.72),
    (11.52, 3.15), (8.63, 0.83), (8.56, 1.39), (8.56, 2.26), (8.66, 2.99), (8.93, 3.85),
    (9.36, 4.65), (9.69, 5.08), (10.06, 5.44), (10.79, 6.01), (11.82, 6.61), (11.78, 6.67),
    (11.29, 7.04), (11.19, 7.04), (10.66, 6.77), (9.66, 6.07), (9.00, 5.41), (8.66, 4.98),
    (8.20, 4.12), (8.13, 4.15), (8.07, 4.12), (5.88, 4.12), (5.84, 4.15), (5.78, 4.12),
    (5.74, 4.15), (5.51, 4.12), (5.44, 4.15), (5.31, 4.12), (4.95, 4.12), (4.91, 4.15),
    (4.81, 4.12), (3.85, 4.12), (3.82, 4.15), (3.75, 4.12), (3.32, 4.12), (3.29, 4.15),
    (3.19, 4.12), (1.43, 4.12), (1.10, 4.15), (0.63, 4.35), (0.23, 4.75), (0.03, 5.21),
    (0.03, 6.24), (0.00, 6.27), (0.03, 6.51), (0.03, 7.04), (0.00, 7.07), (0.03, 7.37),
    (0.03, 11.15), (0.00, 11.19), (0.03, 11.22), (0.03, 11.72), (0.00, 11.75), (0.03, 11.85),
    (0.03, 12.22), (0.00, 12.25), (0.03, 13.18), (0.00, 13.41), (0.03, 13.64), (0.00, 13.88),
    (0.07, 14.24), (0.17, 14.47), (0.53, 14.90), (0.80, 15.07), (1.10, 15.17), (10.42, 15.17),
    (10.79, 15.04), (11.22, 14.67), (11.42, 14.31), (11.49, 14.07), (11.49, 9.53), (10.92, 9.76),
    (10.92, 14.01), (10.82, 14.24), (10.56, 14.51), (10.32, 14.61), (1.20, 14.61), (0.96, 14.51),
    (0.70, 14.24), (0.60, 14.01), (0.60, 5.31), (0.70, 5.08), (0.96, 4.81), (1.20, 4.71),
    (7.77, 4.71), (8.00, 5.54), (8.20, 5.94), (8.50, 6.37), (8.86, 6.74), (9.49, 7.14),
    (10.02, 7.34), (10.59, 7.44), (10.62, 7.50), (9.49, 8.27), (6.71, 10.02), (6.84, 10.06),
    (8.00, 10.02), (9.16, 9.76), (10.09, 9.46), (11.12, 9.06), (12.51, 8.40), (13.24, 7.90),
    (13.84, 7.30), (14.17, 6.80), (14.64, 5.88), (15.14, 5.34), (15.50, 5.11), (15.87, 4.98),
    (16.00, 4.98),
];

/// Grosor del trazo en px CSS (en la grilla de 16).
const STROKE: f32 = 1.5;

struct Pen {
    ops: Vec<PathOp>,
    x: f32,
    y: f32,
    k: f32,
}

impl Pen {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.k, self.y + y * self.k)
    }
    fn m(&mut self, x: f32, y: f32) -> &mut Self {
        let (x, y) = self.p(x, y);
        self.ops.push(PathOp::MoveTo(x, y));
        self
    }
    fn l(&mut self, x: f32, y: f32) -> &mut Self {
        let (x, y) = self.p(x, y);
        self.ops.push(PathOp::LineTo(x, y));
        self
    }
    fn c(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) -> &mut Self {
        let (x1, y1) = self.p(x1, y1);
        let (x2, y2) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.ops.push(PathOp::CubicTo(x1, y1, x2, y2, x, y));
        self
    }
    /// Arco de circunferencia (ángulos en grados, sentido horario en pantalla) como polilínea.
    fn arc(&mut self, cx: f32, cy: f32, r: f32, from: f32, to: f32, move_first: bool) -> &mut Self {
        let steps = 28;
        for i in 0..=steps {
            let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
            let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
            if i == 0 && move_first {
                self.m(x, y);
            } else {
                self.l(x, y);
            }
        }
        self
    }
}

/// Los items que dibujan `icon` dentro de `rect` en `color`.
pub fn icon(icon: Icon, rect: Rect, color: Rgba) -> Vec<DisplayItem> {
    let k = rect.width.min(rect.height) / 16.0;
    let mut pen = Pen {
        ops: Vec::new(),
        x: rect.x + (rect.width - 16.0 * k) / 2.0,
        y: rect.y + (rect.height - 16.0 * k) / 2.0,
        k,
    };
    match icon {
        Icon::Back => {
            pen.m(12.5, 8.0).l(3.5, 8.0);
            pen.m(7.5, 4.0).l(3.5, 8.0).l(7.5, 12.0);
        }
        Icon::Forward => {
            pen.m(3.5, 8.0).l(12.5, 8.0);
            pen.m(8.5, 4.0).l(12.5, 8.0).l(8.5, 12.0);
        }
        Icon::Reload => {
            // Tres cuartos de vuelta y la punta de flecha al final, arriba a la derecha.
            pen.arc(8.0, 8.5, 5.0, -50.0, 250.0, true);
            pen.m(13.2, 2.9).l(12.0, 4.7).l(9.9, 4.3);
        }
        Icon::Search => {
            pen.arc(7.0, 7.0, 4.5, 0.0, 360.0, true);
            pen.m(10.3, 10.3).l(13.5, 13.5);
        }
        Icon::Plus => {
            pen.m(8.0, 3.0).l(8.0, 13.0);
            pen.m(3.0, 8.0).l(13.0, 8.0);
        }
        Icon::Stop | Icon::Close => {
            pen.m(4.0, 4.0).l(12.0, 12.0);
            pen.m(12.0, 4.0).l(4.0, 12.0);
        }
        Icon::Lock => {
            pen.m(5.0, 7.5)
                .l(5.0, 5.5)
                .c(5.0, 1.8, 11.0, 1.8, 11.0, 5.5)
                .l(11.0, 7.5);
            pen.m(3.5, 7.5)
                .l(12.5, 7.5)
                .l(12.5, 14.0)
                .l(3.5, 14.0)
                .l(3.5, 7.5);
            pen.ops.push(PathOp::Close);
        }
        Icon::Engine => {
            pen.arc(8.0, 10.5, 6.0, 180.0, 360.0, true);
            pen.m(8.0, 10.5).l(11.0, 6.5);
            pen.m(2.0, 13.5).l(14.0, 13.5);
        }
        Icon::Brand => {
            for (i, &(x, y)) in BRAND.iter().enumerate() {
                if i == 0 {
                    pen.m(x, y);
                } else {
                    pen.l(x, y);
                }
            }
            pen.ops.push(PathOp::Close);
            return vec![DisplayItem::Path {
                ops: pen.ops,
                fill: Some(color),
                stroke: None,
            }];
        }
        Icon::Globe => {
            pen.arc(8.0, 8.0, 6.0, 0.0, 360.0, true);
            pen.m(8.0, 2.0).c(5.0, 4.5, 5.0, 11.5, 8.0, 14.0);
            pen.m(8.0, 2.0).c(11.0, 4.5, 11.0, 11.5, 8.0, 14.0);
            pen.m(2.2, 8.0).l(13.8, 8.0);
        }
        Icon::Insecure => {
            pen.arc(8.0, 8.0, 6.0, 0.0, 360.0, true);
            pen.m(8.0, 4.8).l(8.0, 8.6);
            pen.m(8.0, 11.0).l(8.0, 11.2);
        }
    }
    vec![DisplayItem::Path {
        ops: pen.ops,
        fill: None,
        stroke: Some((STROKE * k, color)),
    }]
}

/// El indicador de carga de una pestaña: un cuarto de vuelta en `color` sobre la
/// circunferencia tenue, girado según `phase` (0 a 1).
pub fn spinner(rect: Rect, phase: f32, color: Rgba, track: Rgba) -> Vec<DisplayItem> {
    let k = rect.width.min(rect.height) / 16.0;
    let pen = || Pen {
        ops: Vec::new(),
        x: rect.x + (rect.width - 16.0 * k) / 2.0,
        y: rect.y + (rect.height - 16.0 * k) / 2.0,
        k,
    };
    let mut ring = pen();
    ring.arc(8.0, 8.0, 5.5, 0.0, 360.0, true);
    let mut arc = pen();
    let from = phase * 360.0 - 90.0;
    arc.arc(8.0, 8.0, 5.5, from, from + 100.0, true);
    vec![
        DisplayItem::Path {
            ops: ring.ops,
            fill: None,
            stroke: Some((2.0 * k, track)),
        },
        DisplayItem::Path {
            ops: arc.ops,
            fill: None,
            stroke: Some((2.0 * k, color)),
        },
    ]
}
