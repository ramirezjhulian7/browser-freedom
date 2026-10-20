//! Iconos de la interfaz dibujados como paths (líneas de 1.5 px con extremos y uniones
//! redondeados) en una grilla de 16×16 escalada al rectángulo pedido. Nada de fuentes de
//! iconos.

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
}

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
        Icon::Stop => {
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
    }
    vec![DisplayItem::Path {
        ops: pen.ops,
        fill: None,
        stroke: Some((STROKE * k, color)),
    }]
}
