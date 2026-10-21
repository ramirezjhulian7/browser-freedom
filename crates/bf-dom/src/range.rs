//! Rangos vivos (DOM §5.5) — **E3**. El documento guarda los boundary points de cada `Range`
//! y los actualiza en las mutaciones del árbol: "insert" (§4.2.3 paso 5), "remove" (§4.2.3
//! pasos 4-7), "replace data" (§4.10 pasos 8-11), "split a Text node" (§4.11 paso 7) y
//! `normalize()` (§4.4 paso 6). Los algoritmos de `Range` (comparar, extraer, clonar…) están
//! en `bf-web`, que es quien los expone.

use crate::{Document, NodeId};

/// Un boundary point (§5.2): un nodo y un offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundaryPoint {
    pub node: NodeId,
    pub offset: u32,
}

/// El inicio y el final de un rango vivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeBounds {
    pub start: BoundaryPoint,
    pub end: BoundaryPoint,
}

/// El identificador de un rango vivo de un documento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangeId(pub u32);

/// Los rangos vivos de un documento.
#[derive(Debug, Clone, Default)]
pub struct LiveRanges {
    slots: Vec<Option<RangeBounds>>,
    free: Vec<u32>,
}

impl LiveRanges {
    fn each(&mut self, mut f: impl FnMut(&mut RangeBounds)) {
        for r in self.slots.iter_mut().flatten() {
            f(r);
        }
    }
}

impl Document {
    /// Registra un rango vivo.
    pub fn create_live_range(&mut self, bounds: RangeBounds) -> RangeId {
        let r = &mut self.live_ranges;
        if let Some(i) = r.free.pop() {
            r.slots[i as usize] = Some(bounds);
            return RangeId(i);
        }
        r.slots.push(Some(bounds));
        RangeId((r.slots.len() - 1) as u32)
    }

    /// Los boundary points de un rango vivo.
    pub fn live_range(&self, id: RangeId) -> RangeBounds {
        self.live_ranges.slots[id.0 as usize].expect("rango vivo liberado")
    }

    pub fn set_live_range(&mut self, id: RangeId, bounds: RangeBounds) {
        self.live_ranges.slots[id.0 as usize] = Some(bounds);
    }

    /// Libera un rango (su objeto ya no existe o pasó a otro documento).
    pub fn drop_live_range(&mut self, id: RangeId) {
        if let Some(slot) = self.live_ranges.slots.get_mut(id.0 as usize)
            && slot.take().is_some()
        {
            self.live_ranges.free.push(id.0);
        }
    }

    /// ¿Hay rangos vivos? (Los ganchos de las mutaciones no hacen nada si no.)
    pub(crate) fn has_live_ranges(&self) -> bool {
        self.live_ranges.slots.len() > self.live_ranges.free.len()
    }

    /// Olvida todos los rangos (un documento clonado no comparte los rangos del original).
    pub fn clear_live_ranges(&mut self) {
        self.live_ranges = LiveRanges::default();
    }

    /// "Insert" (§4.2.3 paso 5): `count` nodos van a `parent` antes del hijo de índice
    /// `index`.
    pub(crate) fn ranges_insert(&mut self, parent: NodeId, index: u32, count: u32) {
        self.live_ranges.each(|r| {
            for bp in [&mut r.start, &mut r.end] {
                if bp.node == parent && bp.offset > index {
                    bp.offset += count;
                }
            }
        });
    }

    /// "Remove" (§4.2.3 pasos 4-7), antes de desvincular `node` (índice `index` en
    /// `parent`).
    pub(crate) fn ranges_remove(&mut self, node: NodeId, parent: NodeId, index: u32) {
        if self.live_ranges.slots.is_empty() {
            return;
        }
        let mut ranges = std::mem::take(&mut self.live_ranges);
        ranges.each(|r| {
            for bp in [&mut r.start, &mut r.end] {
                if self.is_inclusive_ancestor(node, bp.node) {
                    *bp = BoundaryPoint {
                        node: parent,
                        offset: index,
                    };
                } else if bp.node == parent && bp.offset > index {
                    bp.offset -= 1;
                }
            }
        });
        self.live_ranges = ranges;
    }

    /// "Replace data" (§4.10 pasos 8-11).
    pub(crate) fn ranges_replace_data(&mut self, node: NodeId, offset: u32, count: u32, len: u32) {
        self.live_ranges.each(|r| {
            for bp in [&mut r.start, &mut r.end] {
                if bp.node != node {
                    continue;
                }
                if bp.offset > offset && bp.offset <= offset + count {
                    bp.offset = offset;
                } else if bp.offset > offset + count {
                    bp.offset = bp.offset + len - count;
                }
            }
        });
    }

    /// "Split a Text node" (§4.11 paso 7.2-7.5): `new_node` ya está en `parent`, después de
    /// `node` (de índice `index`).
    pub(crate) fn ranges_split(
        &mut self,
        node: NodeId,
        new_node: NodeId,
        offset: u32,
        parent: NodeId,
        index: u32,
    ) {
        self.live_ranges.each(|r| {
            for bp in [&mut r.start, &mut r.end] {
                if bp.node == node && bp.offset > offset {
                    *bp = BoundaryPoint {
                        node: new_node,
                        offset: bp.offset - offset,
                    };
                } else if bp.node == parent && bp.offset == index + 1 {
                    bp.offset += 1;
                }
            }
        });
    }

    /// `normalize()` (§4.4 paso 6.6.1-4): `current` (de índice `index` en `parent`) se fusiona
    /// en `node`, cuyos datos ya medían `length` antes de él.
    pub(crate) fn ranges_merge(
        &mut self,
        node: NodeId,
        current: NodeId,
        parent: NodeId,
        index: u32,
        length: u32,
    ) {
        self.live_ranges.each(|r| {
            for bp in [&mut r.start, &mut r.end] {
                if bp.node == current {
                    *bp = BoundaryPoint {
                        node,
                        offset: bp.offset + length,
                    };
                } else if bp.node == parent && bp.offset == index {
                    *bp = BoundaryPoint {
                        node,
                        offset: length,
                    };
                }
            }
        });
    }
}
