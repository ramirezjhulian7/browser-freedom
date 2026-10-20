//! Floats de un block formatting context: colocación según las reglas de CSS 2.2 §9.5.1,
//! `clear` y el espacio disponible para line boxes y BFC roots junto a los floats.

use bf_style::properties::types::Clear;

use crate::fragment::Rect;

#[derive(Debug, Clone, Copy)]
pub struct PlacedFloat {
    /// Margin box en coordenadas absolutas.
    pub rect: Rect,
    pub left: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FloatContext {
    pub floats: Vec<PlacedFloat>,
    /// Regla 5: el top de un float no puede quedar más arriba que el de uno anterior.
    last_top: f32,
}

impl FloatContext {
    pub fn is_empty(&self) -> bool {
        self.floats.is_empty()
    }

    fn overlapping(&self, y: f32, height: f32) -> impl Iterator<Item = &PlacedFloat> {
        // Una banda de altura 0 se trata como de altura mínima para detectar floats en `y`.
        let bottom = y + height.max(0.01);
        self.floats
            .iter()
            .filter(move |f| f.rect.y < bottom && f.rect.bottom() > y && f.rect.height > 0.0)
    }

    /// Espacio horizontal libre en `[y, y + height)` dentro de `[left, right]`.
    pub fn band(&self, y: f32, height: f32, left: f32, right: f32) -> (f32, f32) {
        let mut l = left;
        let mut r = right;
        for f in self.overlapping(y, height) {
            if f.left {
                l = l.max(f.rect.right());
            } else {
                r = r.min(f.rect.x);
            }
        }
        (l, r)
    }

    /// El siguiente `y` por debajo de `y` donde termina algún float que se superpone a la banda.
    fn next_y(&self, y: f32, height: f32) -> Option<f32> {
        self.overlapping(y, height)
            .map(|f| f.rect.bottom())
            .filter(|b| *b > y)
            .min_by(f32::total_cmp)
    }

    /// El borde inferior más cercano por debajo de `y` de los floats que se superponen a la
    /// banda `[y, y + height)`.
    pub fn next_bottom(&self, y: f32, height: f32) -> Option<f32> {
        self.next_y(y, height)
    }

    /// Coloca un float de margin box `width`×`height`; devuelve la esquina superior izquierda
    /// del margin box. `y_min` es el top de la line box o del bloque actual (reglas 4 y 6).
    pub fn place(
        &mut self,
        left: bool,
        width: f32,
        height: f32,
        y_min: f32,
        cb_left: f32,
        cb_right: f32,
    ) -> (f32, f32) {
        let mut y = y_min.max(self.last_top);
        loop {
            let (l, r) = self.band(y, height, cb_left, cb_right);
            let fits = r - l >= width - 0.001;
            let any = self.overlapping(y, height).next().is_some();
            if fits || !any {
                let x = if left { l } else { r - width };
                self.floats.push(PlacedFloat {
                    rect: Rect::new(x, y, width, height),
                    left,
                });
                self.last_top = y;
                return (x, y);
            }
            match self.next_y(y, height) {
                Some(next) => y = next,
                None => y += 1.0,
            }
        }
    }

    /// Borde inferior de los floats que `clear` despeja.
    pub fn clear_y(&self, clear: Clear) -> Option<f32> {
        let relevant = |f: &&PlacedFloat| match clear {
            Clear::None => false,
            Clear::Left => f.left,
            Clear::Right => !f.left,
            Clear::Both => true,
        };
        self.floats
            .iter()
            .filter(relevant)
            .map(|f| f.rect.bottom())
            .max_by(f32::total_cmp)
    }

    /// Borde inferior de todos los floats (para la altura `auto` de un BFC root).
    pub fn bottom(&self) -> Option<f32> {
        self.floats
            .iter()
            .map(|f| f.rect.bottom())
            .max_by(f32::total_cmp)
    }
}
