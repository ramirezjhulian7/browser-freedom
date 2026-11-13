//! Las imágenes `loading=lazy` que el host tiene aplazadas (HTML §4.8.4.3.9, "lazy loading
//! attributes"). **E2.**
//!
//! Quien decide si una imagen se aplaza y cuándo se pide es el motor web (`bf-web`, con el
//! "lazy load intersection observer" y el margen de cercanía al viewport); el pintado
//! (`bf-paint`) trabaja sobre una copia del documento y no tiene ese estado. El motor anota
//! aquí cada `<img>` que espera (aplazado o con la petición reanudada aún en vuelo) y quita
//! la anotación cuando la imagen llega o falla: el pintado no pide las anotadas, así que una
//! imagen perezosa lejana no genera ninguna petición, y cuando el motor ya la tiene, el
//! pintado la encuentra en la caché en lugar de descargarla otra vez. Cada cambio sube la
//! versión del documento, para que el navegador vuelva a maquetar y la pinte.

use crate::{Document, NodeId};

impl Document {
    /// ¿El host tiene aplazada la imagen de `node` (`loading=lazy`)?
    pub fn image_deferred(&self, node: NodeId) -> bool {
        self.deferred_images.contains(&node)
    }

    /// Anota (`true`) o quita (`false`) que la imagen de `node` está aplazada.
    pub fn set_image_deferred(&mut self, node: NodeId, deferred: bool) {
        let changed = if deferred {
            self.deferred_images.insert(node)
        } else {
            self.deferred_images.remove(&node)
        };
        if changed {
            self.bump_version();
        }
    }
}
