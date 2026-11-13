//! CSS Font Loading 3 (E5): `FontFace`, `FontFaceSet`, `document.fonts`.

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    fn new(body: &str) -> Page {
        let html = format!("<!DOCTYPE html><html><head></head><body>{body}</body></html>");
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (mut doc, _) = bf_html::parse_document(&html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/page.html", None).expect("url");
        doc.url = url.clone();
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, ctx }
    }

    fn show(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        let r = match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        };
        self.rt.run_until_idle().expect("tareas");
        r
    }
}

/// La URL `file:` de la Ahem de WPT.
fn ahem_url() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf");
    let path = std::fs::canonicalize(path).expect("tests/wpt/fonts/Ahem.ttf");
    Url::from_file_path(&path).expect("url").serialize(false)
}

/// Maqueta el documento de la página con las caras de sus hojas (y las de `document.fonts`) y
/// sin fuentes del sistema; devuelve el volcado del árbol de fragmentos.
fn layout_dump(p: &Page) -> String {
    use std::sync::Arc;
    let ctx = p.ctx.get(&p.rt.agent);
    let doc = ctx.document();
    let media = bf_css::media::MediaContext::default();
    let sheets = bf_style::document_stylesheets(doc, &media, None);
    let refs: Vec<&bf_css::stylesheet::Stylesheet> = sheets.iter().collect();
    let mut fonts = bf_font::Fonts::new(Arc::new(bf_font::FontDb::from_faces(Vec::new())));
    fonts.add_font_faces(
        &bf_style::font_face_rules(&refs, &media),
        Arc::new(bf_infra::FileLoader::new()),
    );
    let metrics = bf_layout::StyleMetrics(&fonts);
    let styles = bf_style::cascade::style_document_with_sheets(
        doc,
        &sheets,
        &[],
        &media,
        &bf_css::selectors::MatchContext::for_document(doc),
        Some(&metrics),
    );
    let tree = bf_layout::layout_document(
        doc,
        &styles,
        bf_layout::Size {
            width: 800.0,
            height: 600.0,
        },
        &fonts,
    );
    bf_layout::dump(doc, &tree)
}

#[test]
fn document_fonts_ready() {
    let mut p = Page::new("<p>x</p>");
    let r = p.show(
        "var log = []; document.fonts.ready.then(s => log.push('ready', s === document.fonts)); \
         [typeof FontFace, String(document.fonts), document.fonts.status, document.fonts.size, \
          document.fonts === document.fonts, document.fonts instanceof EventTarget].join()",
    );
    assert_eq!(r, "function,[object FontFaceSet],loaded,0,true,true");
    assert_eq!(p.show("log.join()"), "ready,true");
}

#[test]
fn font_face_descriptors() {
    let mut p = Page::new("");
    let r = p.show(
        "var f = new FontFace('My Font', 'url(a.ttf)', {weight: 'bold', style: 'italic', width: '50%'}); \
         [f.family, f.weight, f.style, f.stretch, f.width, f.unicodeRange, f.display, f.status].join('|')",
    );
    assert_eq!(r, "My Font|bold|italic|50%|50%|U+0-10FFFF|auto|unloaded");
    // Un descriptor inválido en el constructor deja la cara en error; en un setter, lanza.
    let r = p.show(
        "var g = new FontFace('x', 'url(a.ttf)', {weight: 'heavy'}); var e1 = g.status; \
         var e2; try { f.weight = 'nope' } catch (e) { e2 = e.name } \
         var e3; g.loaded.catch(e => e3 = e.name); [e1, e2, f.weight].join()",
    );
    assert_eq!(r, "error,SyntaxError,bold");
    assert_eq!(p.show("e3"), "SyntaxError");
    // Un nombre que no es <family-name> se guarda entre comillas.
    assert_eq!(p.show("new FontFace('sans-serif', 'url(a)').family"), "\"sans-serif\"");
}

#[test]
fn font_face_set_membership_and_events() {
    let url = ahem_url();
    let mut p = Page::new("<p id=t style=\"font: 20px Test\">abc</p>");
    let r = p.show(&format!(
        "var events = []; ['loading', 'loadingdone', 'loadingerror'].forEach(t => \
         document.fonts.addEventListener(t, e => events.push(t + ':' + e.fontfaces.length))); \
         var f = new FontFace('Test', 'url(\"{url}\")'); document.fonts.add(f); \
         var bad = new FontFace('Bad', 'url(\"file:///nonexistent.ttf\")'); document.fonts.add(bad); \
         var res = []; f.load().then(x => res.push(x === f, f.status)); \
         bad.load().catch(e => res.push(e.name, bad.status)); \
         [document.fonts.size, document.fonts.has(f), f.status, document.fonts.status].join()"
    ));
    assert_eq!(r, "2,true,loading,loading");
    assert_eq!(p.show("res.join()"), "true,loaded,NetworkError,error");
    assert_eq!(p.show("events.join()"), "loading:0,loadingdone:1,loadingerror:1");
    assert_eq!(
        p.show("[document.fonts.status, document.fonts.check('20px Test'), [...document.fonts].length].join()"),
        "loaded,true,2"
    );
    assert_eq!(p.show("document.fonts.delete(bad) + ',' + document.fonts.size"), "true,1");
    assert_eq!(
        p.show("try { document.fonts.check('inherit') } catch (e) { e.name }"),
        "SyntaxError"
    );
    // La maqueta usa la cara: tres glifos de Ahem de 20px.
    assert!(
        layout_dump(&p).contains("Text \"abc\" 8,20 60×20"),
        "{}",
        layout_dump(&p)
    );
    p.show("document.fonts.clear()");
    assert!(!layout_dump(&p).contains("60×20"));
}

/// Una `FontFace` con los datos binarios de Ahem (un `ArrayBuffer`), añadida desde un script,
/// se usa en el texto.
#[test]
fn font_face_from_array_buffer_used_by_layout() {
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/fonts/Ahem.ttf"),
    )
    .expect("Ahem.ttf");
    let list: Vec<String> = bytes.iter().map(u8::to_string).collect();
    let mut p = Page::new("<p style=\"font: 10px Bin\">abcd</p>");
    let r = p.show(&format!(
        "var f = new FontFace('Bin', new Uint8Array([{}]).buffer); var s0 = f.status; \
         document.fonts.add(f); var done; document.fonts.ready.then(() => done = f.status); s0",
        list.join(",")
    ));
    assert_eq!(r, "loading");
    assert_eq!(p.show("done"), "loaded");
    assert!(
        layout_dump(&p).contains("Text \"abcd\" 8,10 40×10"),
        "{}",
        layout_dump(&p)
    );
    // Datos que no son una fuente: SyntaxError.
    let r = p.show("var x = new FontFace('X', new Uint8Array([1,2,3]).buffer); var n; x.loaded.catch(e => n = e.name); 0");
    assert_eq!(r, "0");
    assert_eq!(p.show("[n, x.status].join()"), "SyntaxError,error");
}

#[test]
fn css_connected_faces() {
    let url = ahem_url();
    let mut p = Page::new("");
    let r = p.show(&format!(
        "var st = document.createElement('style'); \
         st.textContent = '@font-face {{ font-family: Css; src: url(\"{url}\") }}'; \
         document.head.appendChild(st); \
         var css = [...document.fonts][0]; \
         var r = []; document.fonts.load('12px Css').then(l => r.push(l.length, l[0] === css, css.status)); \
         var e; try {{ document.fonts.add(css) }} catch (x) {{ e = x.name }} \
         [document.fonts.size, css.family, document.fonts.delete(css), e].join()"
    ));
    assert_eq!(r, "1,Css,false,InvalidModificationError");
    assert_eq!(p.show("r.join()"), "1,true,loaded");
}
