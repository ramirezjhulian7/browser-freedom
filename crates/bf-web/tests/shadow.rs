//! Shadow DOM desde JavaScript (E1): `attachShadow`, `ShadowRoot`, slots y `slotchange`, el
//! retargeting de eventos y `composedPath`, `getRootNode`, `getHTML`/`setHTMLUnsafe`,
//! declarative shadow DOM, `adoptedStyleSheets`, `delegatesFocus` y las propiedades con
//! nombre de `Window`.

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
        let opts = bf_html::ParseOptions {
            allow_declarative_shadow_roots: true,
            ..bf_html::ParseOptions::default()
        };
        let (mut doc, _) = bf_html::parse_document(&html, &opts);
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

#[test]
fn attach_shadow() {
    let mut p = Page::new("<div id=host></div><input id=inp>");
    assert_eq!(
        p.show(
            "var sr = host.attachShadow({mode: 'open'}); \
             [sr instanceof ShadowRoot, sr instanceof DocumentFragment, sr.mode, sr.host === host, \
              host.shadowRoot === sr, sr.nodeType, String(sr)].join()"
        ),
        "true,true,open,true,true,11,[object ShadowRoot]"
    );
    // Segundo attach, elemento no permitido y modo inválido.
    assert_eq!(
        p.show("try { host.attachShadow({mode: 'open'}) } catch (e) { e.name }"),
        "NotSupportedError"
    );
    assert_eq!(
        p.show("try { inp.attachShadow({mode: 'open'}) } catch (e) { e.name }"),
        "NotSupportedError"
    );
    assert_eq!(
        p.show(
            "try { document.createElement('div').attachShadow({mode: 'x'}) } catch (e) { e.name }"
        ),
        "TypeError"
    );
    // Cerrado: `shadowRoot` es null.
    assert_eq!(
        p.show(
            "var c = document.createElement('span'); var cr = c.attachShadow({mode: 'closed'}); \
             [c.shadowRoot, cr.mode, cr.slotAssignment, cr.delegatesFocus, cr.clonable].join()"
        ),
        ",closed,named,false,false"
    );
}

#[test]
fn tree_concepts() {
    let mut p = Page::new("<div id=host></div>");
    let r = p.show(
        "var sr = host.attachShadow({mode: 'open'}); sr.innerHTML = '<p id=in>x</p>'; \
         var inner = sr.getElementById('in'); \
         [inner.isConnected, inner.getRootNode() === sr, inner.getRootNode({composed: true}) === document, \
          document.getElementById('in'), sr.querySelector('p') === inner, sr.innerHTML, \
          document.contains(inner), inner.parentNode === sr, sr.parentNode].join()",
    );
    assert_eq!(r, "true,true,true,,true,<p id=\"in\">x</p>,false,true,");
    // Al sacar el host del documento, lo de adentro deja de estar conectado.
    assert_eq!(p.show("host.remove(); inner.isConnected"), "false");
}

#[test]
fn slots_and_slotchange() {
    let mut p = Page::new("<div id=host><span id=a slot=s1>a</span><b id=b>b</b>t</div>");
    let r = p.show(
        "var sr = host.attachShadow({mode: 'open'}); \
         sr.innerHTML = '<slot name=s1 id=s1></slot><slot id=def>fb</slot>'; \
         var s1 = sr.getElementById('s1'), def = sr.getElementById('def'); \
         [s1.assignedNodes().length, def.assignedNodes().length, a.assignedSlot === s1, \
          b.assignedSlot === def, s1.assignedElements()[0] === a].join()",
    );
    assert_eq!(r, "1,2,true,true,true");
    // `slotchange` se dispara en una microtarea, después de los observers.
    let r = p.show(
        "window.log = []; s1.addEventListener('slotchange', e => log.push('s1:' + e.bubbles)); \
         def.addEventListener('slotchange', () => log.push('def')); \
         b.slot = 's1'; log.length",
    );
    assert_eq!(r, "0");
    // El orden del spec: primero el slot que lo tenía, después el nuevo.
    assert_eq!(p.show("log.join()"), "def,s1:true");
    assert_eq!(
        p.show("s1.assignedNodes().length + ',' + def.assignedNodes().length"),
        "2,1"
    );
    // Flatten: el contenido de respaldo cuenta si no hay nodos asignados.
    assert_eq!(
        p.show(
            "var h2 = document.createElement('div'); var r2 = h2.attachShadow({mode: 'open'}); \
             r2.innerHTML = '<slot>fb</slot>'; var sl = r2.firstChild; \
             [sl.assignedNodes().length, sl.assignedNodes({flatten: true}).length].join()"
        ),
        "0,1"
    );
    // Asignación manual.
    let r = p.show(
        "var h3 = document.createElement('div'); h3.innerHTML = '<i>1</i><i>2</i>'; \
         var r3 = h3.attachShadow({mode: 'open', slotAssignment: 'manual'}); \
         r3.innerHTML = '<slot></slot>'; var ms = r3.firstChild; \
         var before = ms.assignedNodes().length; ms.assign(h3.lastChild); \
         [before, ms.assignedNodes()[0] === h3.lastChild, h3.firstChild.assignedSlot].join()",
    );
    assert_eq!(r, "0,true,");
}

#[test]
fn event_retargeting() {
    let mut p = Page::new("<div id=host></div>");
    let r = p.show(
        "var sr = host.attachShadow({mode: 'open'}); sr.innerHTML = '<button id=btn>x</button>'; \
         var btn = sr.getElementById('btn'); var seen = []; \
         document.body.addEventListener('click', e => seen.push('body:' + e.target.id)); \
         sr.addEventListener('click', e => seen.push('sr:' + e.target.id)); \
         btn.addEventListener('click', e => seen.push('btn:' + e.composedPath().length)); \
         btn.dispatchEvent(new MouseEvent('click', {bubbles: true, composed: true})); \
         seen.join()",
    );
    // body, html, document, window + host, sr, btn.
    assert_eq!(r, "btn:7,sr:btn,body:host");
    // Sin `composed`, el evento no sale del shadow tree.
    let r =
        p.show("seen = []; btn.dispatchEvent(new Event('click', {bubbles: true})); seen.join()");
    assert_eq!(r, "btn:2,sr:btn");
    // En un árbol cerrado, `composedPath()` desde afuera no muestra los nodos de adentro.
    let r = p.show(
        "var ch = document.createElement('div'); document.body.append(ch); \
         var cr = ch.attachShadow({mode: 'closed'}); cr.innerHTML = '<i>x</i>'; \
         var path; document.body.addEventListener('ping', e => path = e.composedPath()); \
         cr.firstChild.dispatchEvent(new Event('ping', {bubbles: true, composed: true})); \
         [path[0] === ch, path.length].join()",
    );
    assert_eq!(r, "true,5");
}

#[test]
fn get_html_and_declarative() {
    let mut p = Page::new(
        "<div id=d><template shadowrootmode=open shadowrootserializable><b>in</b></template>light</div>",
    );
    assert_eq!(
        p.show("[d.shadowRoot.innerHTML, d.innerHTML].join('|')"),
        "<b>in</b>|light"
    );
    assert_eq!(
        p.show("d.getHTML({serializableShadowRoots: true})"),
        "<template shadowrootmode=\"open\" shadowrootserializable=\"\"><b>in</b></template>light"
    );
    // `innerHTML` no crea shadow roots; `setHTMLUnsafe` sí.
    let r = p.show(
        "var w = document.createElement('div'); \
         w.innerHTML = '<p><template shadowrootmode=open>x</template></p>'; \
         var a = w.firstChild.shadowRoot; \
         w.setHTMLUnsafe('<p><template shadowrootmode=open>x</template></p>'); \
         [a, w.firstChild.shadowRoot.innerHTML].join()",
    );
    assert_eq!(r, ",x");
    // Un attach imperativo sobre un shadow root declarativo lo vacía y lo devuelve.
    assert_eq!(
        p.show("var s = d.shadowRoot; [d.attachShadow({mode: 'open'}) === s, s.innerHTML].join()"),
        "true,"
    );
    assert_eq!(
        p.show("var nd = Document.parseHTMLUnsafe('<div><template shadowrootmode=open>z</template></div>'); nd.querySelector('div').shadowRoot.innerHTML"),
        "z"
    );
}

#[test]
fn clone_and_adopt() {
    let mut p = Page::new("<div id=host></div>");
    let r = p.show(
        "var sr = host.attachShadow({mode: 'open', clonable: true}); sr.innerHTML = '<i>x</i>'; \
         var c = host.cloneNode(true), s = host.cloneNode(false); \
         [c.shadowRoot.innerHTML, c.shadowRoot.clonable, s.shadowRoot.innerHTML].join()",
    );
    // Los hijos del shadow root se clonan con el mismo *subtree* que el host.
    assert_eq!(r, "<i>x</i>,true,<i></i>");
    assert_eq!(
        p.show("try { sr.cloneNode(true) } catch (e) { e.name }"),
        "NotSupportedError"
    );
    assert_eq!(
        p.show("try { document.adoptNode(sr) } catch (e) { e.name }"),
        "HierarchyRequestError"
    );
    // Adoptado a otro documento, el shadow root sigue siendo el mismo objeto.
    let r = p.show(
        "var other = document.implementation.createHTMLDocument(''); var i = sr.firstChild; \
         var h = host; other.body.append(h); [h.shadowRoot === sr, sr.firstChild === i, i.ownerDocument === other].join()",
    );
    assert_eq!(r, "true,true,true");
}

#[test]
fn adopted_style_sheets() {
    let mut p = Page::new("<div id=host></div>");
    let r = p.show(
        "var s = new CSSStyleSheet(); s.replaceSync('p { color: red }'); \
         var sr = host.attachShadow({mode: 'open'}); sr.adoptedStyleSheets = [s]; \
         [sr.adoptedStyleSheets.length, sr.adoptedStyleSheets[0] === s, document.adoptedStyleSheets.length].join()",
    );
    assert_eq!(r, "1,true,0");
    let r = p.show(
        "var st = document.createElement('style'); document.head.append(st); \
         try { document.adoptedStyleSheets = [st.sheet] } catch (e) { e.name }",
    );
    assert_eq!(r, "NotAllowedError");
}

// E1: `Document.adoptedStyleSheets` lo instala solo el CSSOM de E2 (`document_sheets`); el de
// `dom/shadow.rs` es solo de `ShadowRoot`. La hoja adoptada por el documento llega a la cascada
// por un único camino: no se copia también al nodo documento (`adopted_style_texts`), y quitarla
// del array quita sus estilos.
#[test]
fn document_adopted_sheet_applies_once() {
    let mut p = Page::new("<p id=t>x</p><div id=host></div>");
    let r = p.show(
        "var s = new CSSStyleSheet(); s.replaceSync('p { color: rgb(0, 128, 0) }');          document.adoptedStyleSheets = [s];          var sr = host.attachShadow({mode: 'open'}); sr.innerHTML = '<p id=in>y</p>';          var s2 = new CSSStyleSheet(); s2.replaceSync('p { color: rgb(0, 0, 255) }');          sr.adoptedStyleSheets = [s2];          [document.adoptedStyleSheets.length, document.adoptedStyleSheets[0] === s,           getComputedStyle(t).color, getComputedStyle(sr.getElementById('in')).color].join()",
    );
    assert_eq!(r, "1,true,rgb(0, 128, 0),rgb(0, 0, 255)");
    {
        let ctx = p.ctx.get(&p.rt.agent);
        let d = ctx.document();
        assert!(d.adopted_style_texts(d.root()).is_empty());
    }
    let r = p.show(
        "s.replaceSync('p { color: rgb(255, 0, 0) }'); var a = getComputedStyle(t).color;          document.adoptedStyleSheets = []; [a, getComputedStyle(t).color].join()",
    );
    assert_eq!(r, "rgb(255, 0, 0),rgb(0, 0, 0)");
}

#[test]
fn delegates_focus_and_active_element() {
    let mut p = Page::new("<div id=host></div>");
    let r = p.show(
        "var sr = host.attachShadow({mode: 'open', delegatesFocus: true}); \
         sr.innerHTML = '<span>t</span><input id=f>'; host.focus(); \
         [document.activeElement === host, sr.activeElement === sr.getElementById('f')].join()",
    );
    assert_eq!(r, "true,true");
}

#[test]
fn window_named_properties() {
    let mut p = Page::new("<div id=solo></div><img name=pic><p id=dup></p><p id=dup></p>");
    let r = p.show(
        "[solo === document.getElementById('solo'), pic instanceof HTMLImageElement, \
          dup instanceof HTMLCollection, dup.length, 'solo' in window, \
          Object.keys(window).includes('solo'), typeof nonexistent].join()",
    );
    assert_eq!(r, "true,true,true,2,true,false,undefined");
    // Las variables propias del global ganan.
    assert_eq!(p.show("var solo = 5; solo"), "5");
}
