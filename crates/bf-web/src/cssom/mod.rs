//! CSSOM y CSSOM View: `CSSStyleDeclaration`, `getComputedStyle`, `document.styleSheets` y
//! la geometría (`getBoundingClientRect`, `offset*`, `scroll*`). **Dueño: E2.**
//!
//! - [`style_decl`]: `CSSStyleDeclaration` (CSSOM §6.7) sobre el atributo `style`, sobre las
//!   reglas de las hojas y como declaración computada de solo lectura.
//! - [`computed`]: `getComputedStyle` (CSSOM §9) y los valores resueltos.
//! - [`sheets`]: `StyleSheet`, `CSSStyleSheet`, `StyleSheetList`, `MediaList`, `CSSRule` y
//!   sus subclases (CSSOM §6.1–§6.4).
//! - [`media`]: `matchMedia` y `MediaQueryList` (CSSOM View §4.2).
//! - [`view`]: `DOMRect`, la geometría de `Element` y las propiedades de `Window`
//!   (CSSOM View §4, §6, §7; Geometry Interfaces §4).
//!
//! # De dónde salen los estilos y la maqueta
//!
//! El shell deja en [`crate::WebContext::layout`] un [`LayoutSnapshot`] tras cada layout. Si
//! el snapshot corresponde a la versión actual del documento (y de las hojas del CSSOM) se
//! usa tal cual. Si no hay snapshot (un entorno sin interfaz, los tests), los estilos y la
//! maqueta se calculan aquí bajo demanda con `bf-style` y `bf-layout` (fuente de prueba de
//! métricas fijas, o el [`LayoutProvider`] que instale el embebedor). Si hay un snapshot
//! viejo y ningún proveedor, los estilos se recalculan y la geometría usa el snapshot viejo.

pub mod computed;
pub mod declaration;
pub mod media;
pub mod sheets;
pub mod style_decl;
pub mod value;
pub mod view;

use std::collections::HashMap;
use std::sync::Arc;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_dom::Document;
use bf_js::host::HostKey;
use bf_js::value::{PropertyKey, Value};
use bf_js::vm::{Agent, RealmId};
use bf_layout::{FragmentTree, Size, StyleMetrics, TestFont};
use bf_style::StyleMap;

use crate::{NodeRef, WebContextHandle};

/// El resultado del último layout (lo deja el shell en [`crate::WebContext::layout`]).
#[derive(Clone)]
pub struct LayoutSnapshot {
    /// Los estilos computados con que se maquetó.
    pub styles: Arc<StyleMap>,
    /// El árbol de fragmentos (coordenadas de página, sin scroll).
    pub fragments: Arc<FragmentTree>,
    /// `Document::version()` del documento maquetado.
    pub version: u64,
    /// [`sheets_generation`] de las hojas del CSSOM con que se maquetó.
    pub sheets_generation: u64,
    /// El viewport en px CSS.
    pub viewport: Size,
    /// El desplazamiento del viewport en px CSS.
    pub scroll: (f32, f32),
    pub device_pixel_ratio: f32,
    /// La posición de la ventana en la pantalla (`screenX`, `screenY`).
    pub screen_position: (f32, f32),
    /// El tamaño exterior de la ventana (`outerWidth`, `outerHeight`); 0 = el viewport.
    pub outer_size: (f32, f32),
}

impl std::fmt::Debug for LayoutSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutSnapshot")
            .field("version", &self.version)
            .field("viewport", &self.viewport)
            .field("scroll", &self.scroll)
            .finish_non_exhaustive()
    }
}

impl LayoutSnapshot {
    /// Un snapshot con los valores por defecto de la ventana.
    pub fn new(styles: Arc<StyleMap>, fragments: Arc<FragmentTree>, version: u64) -> Self {
        let viewport = fragments.viewport;
        LayoutSnapshot {
            styles,
            fragments,
            version,
            sheets_generation: 0,
            viewport,
            scroll: (0.0, 0.0),
            device_pixel_ratio: 1.0,
            screen_position: (0.0, 0.0),
            outer_size: (0.0, 0.0),
        }
    }
}

/// Quien maqueta un documento cuando hace falta una maqueta al día (el shell puede instalar
/// uno con sus fuentes reales).
pub trait LayoutProvider {
    fn layout(&mut self, doc: &Document, styles: &StyleMap, viewport: Size) -> FragmentTree;
}

/// El estado del CSSOM de una ventana (dato del realm).
pub struct CssomState {
    pub sheets: sheets::SheetStore,
    /// Sube con cada cambio de las hojas hecho desde el CSSOM.
    pub sheets_generation: u64,
    styles: Option<(u64, u64, Arc<StyleMap>)>,
    layout: Option<(u64, u64, Arc<FragmentTree>)>,
    /// El viewport cuando no hay snapshot.
    pub viewport: Size,
    pub device_pixel_ratio: f32,
    /// El desplazamiento del viewport pedido o conocido.
    pub scroll: (f32, f32),
    /// Un desplazamiento pedido desde JavaScript que el shell aún no aplicó.
    pub scroll_request: Option<(f32, f32)>,
    /// El desplazamiento de los elementos con overflow.
    pub element_scroll: HashMap<NodeRef, (f32, f32)>,
    pub provider: Option<Box<dyn LayoutProvider>>,
}

impl Default for CssomState {
    fn default() -> Self {
        CssomState {
            sheets: sheets::SheetStore::default(),
            sheets_generation: 0,
            styles: None,
            layout: None,
            viewport: Size {
                width: 800.0,
                height: 600.0,
            },
            device_pixel_ratio: 1.0,
            scroll: (0.0, 0.0),
            scroll_request: None,
            element_scroll: HashMap::new(),
            provider: None,
        }
    }
}

/// Las clases host del CSSOM (una vez por agente).
#[derive(Debug, Clone)]
pub struct CssomClasses {
    pub style_declaration: bf_js::host::HostClassId,
    pub style_sheet: bf_js::host::HostClassId,
    pub css_style_sheet: bf_js::host::HostClassId,
    pub style_sheet_list: bf_js::host::HostClassId,
    pub media_list: bf_js::host::HostClassId,
    pub rule_list: bf_js::host::HostClassId,
    pub rules: sheets::RuleClasses,
    pub media_query_list: bf_js::host::HostClassId,
    pub rect_read_only: bf_js::host::HostClassId,
    pub rect: bf_js::host::HostClassId,
    pub rect_list: bf_js::host::HostClassId,
}

/// Las clases del CSSOM del agente.
pub fn classes(agent: &Agent) -> &CssomClasses {
    agent
        .embedder_data::<CssomClasses>()
        .expect("bf_web::cssom::register no se llamó")
}

/// Registra las clases de esta área (lo llama [`crate::register_all`]).
pub fn register(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    if agent.embedder_data::<CssomClasses>().is_none() {
        let style_declaration = agent.register_host_class(style_decl::class());
        let (style_sheet, css_style_sheet, style_sheet_list, media_list, rule_list, rules) =
            sheets::register_classes(agent);
        let media_query_list = media::register_class(agent);
        let (rect_read_only, rect, rect_list) = view::register_classes(agent);
        agent.set_embedder_data(CssomClasses {
            style_declaration,
            style_sheet,
            css_style_sheet,
            style_sheet_list,
            media_list,
            rule_list,
            rules,
            media_query_list,
            rect_read_only,
            rect,
            rect_list,
        });
        // Los partial interfaces sobre las clases del DOM.
        let dom = crate::dom::classes(agent).clone();
        agent.host_extend_class(dom.element, style_decl::element_partial());
        agent.host_extend_class(dom.element, view::element_partial());
        agent.host_extend_class(dom.element, sheets::element_partial());
        agent.host_extend_class(dom.document, sheets::document_partial());
        agent.host_extend_class(dom.document, view::document_partial());
        agent.host_extend_class(dom.window, computed::window_partial());
        agent.host_extend_class(dom.window, media::window_partial());
        agent.host_extend_class(dom.window, view::window_partial());
    }
    if agent.realm_data::<CssomState>(realm).is_none() {
        agent.set_realm_data(realm, CssomState::default());
    }
    style_decl::define_property_accessors(agent, realm);
}

// --- Estado ---

/// El estado del CSSOM del realm en curso.
pub fn state(agent: &mut Agent) -> &mut CssomState {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<CssomState>(realm).is_none() {
        agent.set_realm_data(realm, CssomState::default());
    }
    agent
        .realm_data_mut::<CssomState>(realm)
        .expect("estado del CSSOM")
}

/// La generación de las hojas del CSSOM de `realm` (para [`LayoutSnapshot::sheets_generation`]).
pub fn sheets_generation(agent: &Agent, realm: RealmId) -> u64 {
    agent
        .realm_data::<CssomState>(realm)
        .map_or(0, |s| s.sheets_generation)
}

fn state_of(agent: &mut Agent, realm: RealmId) -> &mut CssomState {
    if agent.realm_data::<CssomState>(realm).is_none() {
        agent.set_realm_data(realm, CssomState::default());
    }
    agent
        .realm_data_mut::<CssomState>(realm)
        .expect("estado del CSSOM")
}

/// Instala el proveedor de maquetas síncronas de `realm`.
pub fn set_layout_provider(agent: &mut Agent, realm: RealmId, provider: Box<dyn LayoutProvider>) {
    let s = state_of(agent, realm);
    s.provider = Some(provider);
    s.layout = None;
}

/// Cambia el viewport de `realm` cuando no hay snapshot (entornos sin interfaz).
pub fn set_viewport(agent: &mut Agent, realm: RealmId, width: f32, height: f32, dpr: f32) {
    let s = state_of(agent, realm);
    s.viewport = Size { width, height };
    s.device_pixel_ratio = dpr;
    s.styles = None;
    s.layout = None;
}

/// El desplazamiento pedido desde JavaScript (`scrollTo`…) que el shell debe aplicar.
pub fn take_scroll_request(agent: &mut Agent, realm: RealmId) -> Option<(f32, f32)> {
    state_of(agent, realm).scroll_request.take()
}

/// Las hojas del documento según el CSSOM (con los cambios de `insertRule` etc.), para que
/// el shell maquete con ellas; `None` si el CSSOM no cambió ninguna hoja.
pub fn modified_document_sheets(
    agent: &mut Agent,
    realm: RealmId,
    media: &MediaContext,
) -> Option<Vec<bf_css::stylesheet::Stylesheet>> {
    if sheets_generation(agent, realm) == 0 {
        return None;
    }
    let saved = agent.current_realm;
    agent.current_realm = realm;
    let r = sheets::document_sheets(agent, media);
    agent.current_realm = saved;
    Some(r)
}

/// El snapshot del shell si está al día con el documento y las hojas.
fn fresh_snapshot(agent: &Agent) -> Option<LayoutSnapshot> {
    let h = WebContextHandle::current(agent);
    let ctx = h.get(agent);
    let snap = ctx.layout.as_ref()?;
    let generation = sheets_generation(agent, h.realm);
    (snap.version == ctx.document().version() && snap.sheets_generation == generation)
        .then(|| snap.clone())
}

/// Las dimensiones de la ventana: viewport, scroll, dpr, posición y tamaño exterior.
#[derive(Debug, Clone, Copy)]
pub struct WindowMetrics {
    pub viewport: Size,
    pub scroll: (f32, f32),
    pub dpr: f32,
    pub screen_position: (f32, f32),
    pub outer_size: (f32, f32),
}

pub fn window_metrics(agent: &mut Agent) -> WindowMetrics {
    let snap = WebContextHandle::current(agent)
        .get(agent)
        .layout
        .as_ref()
        .map(|s| {
            (
                s.viewport,
                s.device_pixel_ratio,
                s.screen_position,
                s.outer_size,
                s.scroll,
            )
        });
    let st = state(agent);
    let (viewport, dpr, screen_position, outer, scroll) = match snap {
        Some((v, d, p, o, s)) => (
            v,
            d,
            p,
            o,
            if st.scroll_request.is_some() {
                st.scroll
            } else {
                s
            },
        ),
        None => (
            st.viewport,
            st.device_pixel_ratio,
            (0.0, 0.0),
            (0.0, 0.0),
            st.scroll,
        ),
    };
    WindowMetrics {
        viewport,
        scroll,
        dpr,
        screen_position,
        outer_size: if outer.0 > 0.0 {
            outer
        } else {
            (viewport.width, viewport.height)
        },
    }
}

/// El contexto de las media queries de la ventana.
pub fn media_context(agent: &mut Agent) -> MediaContext {
    let m = window_metrics(agent);
    MediaContext {
        width: f64::from(m.viewport.width),
        height: f64::from(m.viewport.height),
        device_pixel_ratio: f64::from(m.dpr),
        ..MediaContext::default()
    }
}

/// Los estilos computados del documento de la ventana, al día.
pub fn current_styles(agent: &mut Agent) -> Arc<StyleMap> {
    if let Some(s) = fresh_snapshot(agent) {
        return s.styles;
    }
    let h = WebContextHandle::current(agent);
    let version = h.get(agent).document().version();
    let generation = sheets_generation(agent, h.realm);
    if let Some((v, g, s)) = &state(agent).styles
        && *v == version
        && *g == generation
    {
        return s.clone();
    }
    let media = media_context(agent);
    let doc_sheets = sheets::document_sheets(agent, &media);
    let doc = h.get(agent).document();
    let styles = Arc::new(bf_style::style_document_with_sheets(
        doc,
        &doc_sheets,
        &[],
        &media,
        &MatchContext::for_document(doc),
        Some(&StyleMetrics(&TestFont)),
    ));
    state(agent).styles = Some((version, generation, styles.clone()));
    styles
}

/// La maqueta del documento de la ventana (ver la política en la documentación del módulo).
pub fn current_layout(agent: &mut Agent) -> Arc<FragmentTree> {
    if let Some(s) = fresh_snapshot(agent) {
        return s.fragments;
    }
    let h = WebContextHandle::current(agent);
    let has_provider = state(agent).provider.is_some();
    if !has_provider && let Some(s) = h.get(agent).layout.as_ref() {
        return s.fragments.clone();
    }
    let version = h.get(agent).document().version();
    let generation = sheets_generation(agent, h.realm);
    if let Some((v, g, t)) = &state(agent).layout
        && *v == version
        && *g == generation
    {
        return t.clone();
    }
    let styles = current_styles(agent);
    let viewport = window_metrics(agent).viewport;
    let mut provider = state(agent).provider.take();
    let doc = h.get(agent).document();
    let tree = match provider.as_mut() {
        Some(p) => p.layout(doc, &styles, viewport),
        None => bf_layout::layout_document(doc, &styles, viewport, &TestFont),
    };
    let tree = Arc::new(tree);
    let st = state(agent);
    st.provider = provider;
    st.layout = Some((version, generation, tree.clone()));
    tree
}

/// Las hojas del CSSOM cambiaron: hay que volver a calcular estilos y maqueta.
pub fn sheets_changed(agent: &mut Agent) {
    state(agent).sheets_generation += 1;
    crate::dom::mutated(agent);
}

// --- Utilidades de los wrappers ---

/// Las familias de claves de identidad de los objetos del CSSOM (byte alto de `HostKey`,
/// lejos de las de los nodos, `doc << 32 | node`).
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum KeyKind {
    InlineStyle = 0x81,
    Sheet = 0x82,
    Rule = 0x83,
    RuleStyle = 0x84,
    SheetMedia = 0x85,
    SheetRules = 0x86,
    RuleRules = 0x87,
    RuleMedia = 0x88,
    SheetList = 0x89,
}

/// Una clave de identidad del CSSOM.
pub fn key(kind: KeyKind, id: u64) -> HostKey {
    HostKey(((kind as u64) << 56) | (id & 0x00ff_ffff_ffff_ffff))
}

/// La clave de un nodo dentro de una familia del CSSOM.
pub fn node_key(kind: KeyKind, n: NodeRef) -> HostKey {
    key(kind, n.key().0)
}

/// Reescribe las propiedades indexadas (`0..len`) de un objeto "array-like" vivo
/// (`CSSRuleList`, `StyleSheetList`, `MediaList`, `CSSStyleDeclaration`).
pub fn set_indexed(agent: &mut Agent, o: bf_js::heap::Gc<bf_js::object::Object>, items: &[Value]) {
    let mut i = items.len();
    loop {
        let k = PropertyKey::from(i as u32);
        if agent.heap[o].props.get(&k).is_none() {
            break;
        }
        agent.heap[o].props.remove(&k);
        i += 1;
    }
    for (i, v) in items.iter().enumerate() {
        agent.put_data(o, PropertyKey::from(i as u32), v.clone(), false, true, true);
    }
}
