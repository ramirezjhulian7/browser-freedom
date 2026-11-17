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
pub mod font_loading; // E5: CSS Font Loading 3
pub mod intersection_observer; // E2: Intersection Observer
pub mod media;
pub mod namespace;
pub mod resize_observer; // E2: Resize Observer
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

    /// E3: las hojas con que la cascada del CSSOM acaba de calcular los estilos, para que el
    /// proveedor registre sus `@font-face` (CSS Fonts 4 §4) y mida como el pintado.
    fn style_sheets(
        &mut self,
        _sheets: &Arc<Vec<bf_css::stylesheet::Stylesheet>>,
        _media: &MediaContext,
    ) {
    }
}

/// El estado del CSSOM de una ventana (dato del realm).
pub struct CssomState {
    pub sheets: sheets::SheetStore,
    /// Sube con cada cambio de las hojas hecho desde el CSSOM.
    pub sheets_generation: u64,
    styles: Option<(u64, u64, Arc<StyleMap>)>,
    layout: Option<(u64, u64, Arc<FragmentTree>)>,
    /// E4: los estilos con que se hizo `layout` (ver [`current_layout`]).
    layout_styles: Option<Arc<StyleMap>>,
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
    /// E2: sube cada vez que se olvida la maqueta por un cambio que no sube la versión del
    /// documento (una imagen que terminó de cargar), para [`RenderingKey`].
    pub(crate) layout_epoch: u64,
    /// E2: hay un trabajo esperando a que llegue una fuente web ([`watch_web_fonts`]).
    fonts_watched: bool,
    /// E2: el bloque parseado del atributo `style` de cada elemento con el texto del que salió
    /// (ver `style_decl::read_block`).
    pub(crate) inline_blocks: HashMap<NodeRef, (String, declaration::DeclBlock)>,
}

impl Default for CssomState {
    fn default() -> Self {
        CssomState {
            sheets: sheets::SheetStore::default(),
            sheets_generation: 0,
            styles: None,
            layout: None,
            layout_styles: None,
            viewport: Size {
                width: 800.0,
                height: 600.0,
            },
            device_pixel_ratio: 1.0,
            scroll: (0.0, 0.0),
            scroll_request: None,
            element_scroll: HashMap::new(),
            provider: None,
            layout_epoch: 0,
            fonts_watched: false,
            inline_blocks: HashMap::new(),
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
    /// E2: Intersection Observer y Resize Observer.
    pub intersection_observer: bf_js::host::HostClassId,
    pub intersection_observer_entry: bf_js::host::HostClassId,
    pub resize_observer: bf_js::host::HostClassId,
    pub resize_observer_entry: bf_js::host::HostClassId,
    pub resize_observer_size: bf_js::host::HostClassId,
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
        let (intersection_observer, intersection_observer_entry) =
            intersection_observer::register_classes(agent);
        let (resize_observer, resize_observer_entry, resize_observer_size) =
            resize_observer::register_classes(agent);
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
            intersection_observer,
            intersection_observer_entry,
            resize_observer,
            resize_observer_entry,
            resize_observer_size,
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
    namespace::install(agent, realm);
    namespace::install_indexed_iterators(agent, realm);
    font_loading::install(agent, realm); // E5
}

// --- Estado ---

/// El estado del CSSOM del realm en curso.
/// E2: olvida la geometría guardada (terminó de cargar una imagen y su caja cambia de tamaño).
pub fn invalidate_layout(agent: &mut Agent) {
    let st = state(agent);
    st.layout = None;
    st.layout_epoch += 1;
}

// --- E2: los observadores de *update the rendering* ---

/// Lo que determina la geometría de la ventana: si no cambió desde la última vez, los
/// observadores (`IntersectionObserver`, `ResizeObserver`) no tienen nada nuevo que ver.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderingKey {
    version: u64,
    sheets: u64,
    layout_epoch: u64,
    viewport: (u32, u32),
    scroll: (u32, u32),
    dpr: u32,
    element_scroll: u64,
}

pub(crate) fn rendering_key(agent: &Agent, realm: RealmId) -> RenderingKey {
    let ctx = agent.realm_data::<crate::WebContext>(realm);
    let version = ctx.map_or(0, |c| c.document().version());
    let snap = ctx.and_then(|c| c.layout.as_ref());
    let st = agent.realm_data::<CssomState>(realm);
    let (viewport, scroll, dpr) = match (snap, st) {
        (Some(s), Some(st)) if st.scroll_request.is_some() => {
            (s.viewport, st.scroll, s.device_pixel_ratio)
        }
        (Some(s), _) => (s.viewport, s.scroll, s.device_pixel_ratio),
        (None, Some(st)) => (st.viewport, st.scroll, st.device_pixel_ratio),
        (None, None) => (Size::default(), (0.0, 0.0), 1.0),
    };
    let mut element_scroll = 0u64;
    if let Some(st) = st {
        for (n, (x, y)) in &st.element_scroll {
            element_scroll = element_scroll
                .wrapping_mul(31)
                .wrapping_add(n.key().0 ^ u64::from(x.to_bits()) ^ (u64::from(y.to_bits()) << 32));
        }
    }
    RenderingKey {
        version,
        sheets: sheets_generation(agent, realm),
        layout_epoch: st.map_or(0, |s| s.layout_epoch),
        viewport: (viewport.width.to_bits(), viewport.height.to_bits()),
        scroll: (scroll.0.to_bits(), scroll.1.to_bits()),
        dpr: dpr.to_bits(),
        element_scroll,
    }
}

/// E2: los pasos de los observadores en *update the rendering* (HTML §8.1.7.3) de la ventana
/// `realm`, después de los callbacks de `requestAnimationFrame` y de maquetar: los de
/// `ResizeObserver` (paso 14, Resize Observer §3.4.1) y "run the update intersection
/// observations steps" (paso 19, Intersection Observer §3.2.8). Con navegador, si su maqueta
/// del documento de ahora no llegó, esperan a ella (el navegador los corre en
/// `Engine::set_layout`) en vez de maquetar a medio cargar.
pub fn update_observations(agent: &mut Agent, realm: RealmId) {
    if awaiting_shell_layout(agent, realm) {
        return;
    }
    resize_observer::run_steps(agent, realm);
    intersection_observer::update_observations(agent, realm);
}

/// E2: ¿la ventana `realm` necesita un cuadro para sus observadores? (Una observación nueva, o
/// cambió algo de lo que depende la geometría desde la última vez.)
pub fn observations_pending(agent: &Agent, realm: RealmId) -> bool {
    let any = agent
        .realm_data::<intersection_observer::IoState>(realm)
        .is_some()
        || agent
            .realm_data::<resize_observer::RoState>(realm)
            .is_some();
    if !any || awaiting_shell_layout(agent, realm) {
        return false;
    }
    let key = rendering_key(agent, realm);
    resize_observer::pending(agent, realm, &key)
        || intersection_observer::pending(agent, realm, &key)
}

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
    // E2: en la ventana del navegador (red en hilos), las fuentes web llegan en segundo plano
    // (CSS Fonts 4 §4.9, CSS Font Loading 3 §2.2): la maqueta no las espera y, cuando llega
    // una, la página se vuelve a pintar ([`watch_web_fonts`]).
    if !state_of(agent, realm).fonts_watched && crate::net::is_threaded(agent, realm) {
        state_of(agent, realm).fonts_watched = true;
        bf_font::set_async_web_fonts(true);
        watch_web_fonts(agent, realm);
    }
}

/// E2: un trabajo de red que espera a que termine de cargarse alguna fuente web
/// (`bf_font::wait_web_fonts_change`) y, en una tarea de la ventana `realm`, olvida su
/// geometría y marca el documento para volver a pintar (con la fuente ya en la caché). Se
/// vuelve a armar mientras exista la ventana; cada espera dura como mucho 30 s.
fn watch_web_fonts(agent: &mut Agent, realm: RealmId) {
    let since = bf_font::web_fonts_generation();
    let job = Box::new(move || {
        let now = bf_font::wait_web_fonts_change(since, std::time::Duration::from_secs(30));
        Ok(crate::net::NetResponse {
            url_list: Vec::new(),
            status: if now > since { 200 } else { 204 },
            status_text: String::new(),
            headers: crate::net::headers::HeaderList(Vec::new()),
            body: Vec::new(),
        })
    });
    let done = Box::new(move |agent: &mut Agent, r: crate::net::NetResult| {
        if agent.realm_data::<crate::WebContext>(realm).is_none() {
            return;
        }
        if r.is_ok_and(|res| res.status == 200) {
            let s = state_of(agent, realm);
            s.styles = None;
            s.layout = None;
            s.layout_epoch += 1;
            if let Some(ctx) = agent.realm_data_mut::<crate::WebContext>(realm) {
                ctx.mutated();
            }
        }
        watch_web_fonts(agent, realm);
    });
    crate::net::spawn_job(agent, realm, job, done);
}

/// E1: el documento de la ventana `realm` fue reemplazado por otro en el mismo realm (un marco
/// que navegó): los estilos y la maqueta en caché son del anterior y sus `NodeId` no valen en
/// el nuevo (ver `dom::adopt::retire_window_document`).
pub fn document_replaced(agent: &mut Agent, realm: RealmId, to: crate::DocIndex) {
    let s = state_of(agent, realm);
    s.styles = None;
    s.layout = None;
    s.sheets.retire_window_document(to);
}

/// Cambia el viewport de `realm` cuando no hay snapshot (entornos sin interfaz).
pub fn set_viewport(agent: &mut Agent, realm: RealmId, width: f32, height: f32, dpr: f32) {
    let s = state_of(agent, realm);
    s.viewport = Size { width, height };
    s.device_pixel_ratio = dpr;
    s.styles = None;
    s.layout = None;
    // CSSOM View §4.2: las `MediaQueryList` que cambiaron disparan `change`.
    media::queue_report(agent, realm);
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
    Some(Arc::unwrap_or_clone(r))
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

/// E3: ¿la ventana `realm` maqueta con el navegador (tiene proveedor) y ni la maqueta que el
/// navegador le pasó ni la que el CSSOM tiene en caché son del documento de ahora? Entonces
/// pedir una geometría maqueta el documento entero, y la del navegador está por llegar.
pub fn awaiting_shell_layout(agent: &Agent, realm: RealmId) -> bool {
    let Some(st) = agent.realm_data::<CssomState>(realm) else {
        return false;
    };
    if st.provider.is_none() {
        return false;
    }
    let Some(ctx) = agent.realm_data::<crate::WebContext>(realm) else {
        return false;
    };
    let version = ctx.document().version();
    let generation = sheets_generation(agent, realm);
    let shell = ctx
        .layout
        .as_ref()
        .is_some_and(|s| s.version == version && s.sheets_generation == generation);
    let cached = st
        .layout
        .as_ref()
        .is_some_and(|(v, g, _)| *v == version && *g == generation);
    !shell && !cached
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
    // E6: `scripting` (Media Queries 5 §8.1): el documento de la ventana lo sabe.
    let scripting = WebContextHandle::current(agent)
        .get(agent)
        .document()
        .scripting;
    MediaContext {
        width: f64::from(m.viewport.width),
        height: f64::from(m.viewport.height),
        device_pixel_ratio: f64::from(m.dpr),
        scripting: bf_css::media::Scripting::of(scripting),
        ..MediaContext::default()
    }
}

/// Los estilos computados del documento de la ventana, al día.
///
/// E4: con las animaciones y transiciones CSS de la ventana aplicadas en la hora actual
/// (`crate::bom::animation::animate`, el "style change event").
pub fn current_styles(agent: &mut Agent) -> Arc<StyleMap> {
    let base = cascade_styles(agent);
    crate::bom::animation::animate(agent, base)
}

/// Los estilos de la cascada (sin animaciones) del documento de la ventana.
fn cascade_styles(agent: &mut Agent) -> Arc<StyleMap> {
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
    if let Some(p) = state(agent).provider.as_mut() {
        p.style_sheets(&doc_sheets, &media);
    }
    let previous = state(agent).styles.take();
    let doc = h.get(agent).document();
    let ctx = MatchContext::for_document(doc);
    // E2: si desde la cascada anterior (con las mismas hojas) solo cambiaron atributos, se
    // recalculan esos elementos y sus descendientes a partir del estilo guardado de su padre
    // (`bf_style::restyle_after_attribute_changes`, mismo resultado que la completa). En
    // www.rae.es, 50 de las 64 cascadas de una carga eran de ese caso.
    let incremental = previous
        .filter(|(_, g, _)| *g == generation && incremental_style())
        .and_then(|(v, _, old)| {
            let changed = doc.attribute_changes_since(v)?;
            bf_style::restyle_after_attribute_changes(
                doc,
                &doc_sheets,
                &[],
                &media,
                &ctx,
                Some(&TestMetrics),
                &old,
                &changed,
            )
        });
    // E2: en varios hilos, como la cascada del pintado (`bf_style::style_document_parallel`,
    // mismo resultado). En el barrido, cada consulta de geometría de un script que llegaba
    // tras un cambio del DOM rehacía la cascada en un hilo: en www.rae.es, 60 veces de
    // 250–300 ms de emparejar selectores.
    if incremental.is_some() {
        INCREMENTAL_CASCADES.with(|c| c.set(c.get() + 1));
    }
    let styles = Arc::new(incremental.unwrap_or_else(|| {
        bf_style::style_document_parallel(
            doc,
            &doc_sheets,
            &[],
            &media,
            &ctx,
            Some(&TestMetrics),
            cascade_threads(),
        )
    }));
    state(agent).styles = Some((version, generation, styles.clone()));
    styles
}

thread_local! {
    static INCREMENTAL_CASCADES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// E2: cuántas cascadas del CSSOM de este hilo fueron incrementales (para los tests).
pub fn incremental_cascades() -> u64 {
    INCREMENTAL_CASCADES.with(std::cell::Cell::get)
}

/// E2: ¿cascada incremental tras cambios de atributo? Sí, salvo con `BF_NO_INCREMENTAL_STYLE`
/// (para comparar).
fn incremental_style() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BF_NO_INCREMENTAL_STYLE").is_none())
}

/// E2: las métricas de `TestFont` para `ex`/`ch` (las de la cascada del CSSOM), compartibles
/// entre hilos.
struct TestMetrics;

impl bf_style::FontMetricsSource for TestMetrics {
    fn ex_ch(&self, style: &bf_style::ComputedStyle) -> (f32, f32) {
        bf_style::FontMetricsSource::ex_ch(&StyleMetrics(&TestFont), style)
    }
}

/// E2: hilos de la cascada del CSSOM (`BF_CASCADE_THREADS`, o los núcleos hasta 4, como el
/// pintado).
fn cascade_threads() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("BF_CASCADE_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(1, std::num::NonZeroUsize::get)
                    .min(4)
            })
            .max(1)
    })
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
    // E2: para medir (`docs/rendimiento.md`): cada maqueta que hace el CSSOM por su cuenta,
    // con su cascada si hace falta.
    let traced = std::env::var_os("BF_TRACE")
        .is_some()
        .then(std::time::Instant::now);
    // E4: los estilos mostrados (con animaciones y transiciones) también son parte de la
    // clave: mientras algo se anima cambian sin que cambie el documento, y la geometría
    // (`getBoundingClientRect`, el valor resuelto de `width`…) debe seguirlos.
    let styles = current_styles(agent);
    let st = state(agent);
    if let Some((v, g, t)) = &st.layout
        && *v == version
        && *g == generation
        && st
            .layout_styles
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &styles))
    {
        return t.clone();
    }
    let viewport = window_metrics(agent).viewport;
    let mut provider = state(agent).provider.take();
    // E2: sin navegador, las imágenes que ya cargó el documento (`img` con su tamaño).
    let images = if provider.is_none() {
        crate::html::legacy_factories::layout_image_sizes(agent)
    } else {
        bf_layout::ImageSizes::new()
    };
    let doc = h.get(agent).document();
    let mut tree = match provider.as_mut() {
        Some(p) => p.layout(doc, &styles, viewport),
        None => bf_layout::layout_document_with_images(doc, &styles, viewport, &TestFont, &images),
    };
    state(agent).provider = provider;
    view::restore_element_scroll(agent, &mut tree); // E12
    if let Some(t) = traced {
        eprintln!(
            "perf maqueta del CSSOM: {:.1} ms (cascada y maqueta propias)",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
    let tree = Arc::new(tree);
    let st = state(agent);
    st.layout = Some((version, generation, tree.clone()));
    st.layout_styles = Some(styles);
    tree
}

/// Las hojas del CSSOM cambiaron: hay que volver a calcular estilos y maqueta.
pub fn sheets_changed(agent: &mut Agent) {
    state(agent).sheets_generation += 1;
    // E1: las `adoptedStyleSheets` siguen a sus hojas.
    crate::dom::shadow::sync_adopted(agent);
    // E2: y lo que pinta el navegador, a la hoja viva (`sheets::flush_paint_sheets`).
    sheets::schedule_paint_flush(agent);
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
