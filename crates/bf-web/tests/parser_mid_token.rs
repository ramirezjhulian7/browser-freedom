//! E1: el parser corre script en mitad de un token. *Create an element for a token* construye
//! los custom elements de forma síncrona (HTML §13.2.6.1, *will execute script*) y un nodo que
//! el parser inserta en un `script` conectado que no insertó él lo ejecuta en el acto (sus
//! *children changed steps*, §4.12.1.1).

use std::collections::HashMap;
use std::sync::Arc;

use bf_infra::{Resource, ResourceLoader};
use bf_js::host::DefaultHooks;
use bf_web::bom;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct MemLoader(HashMap<String, String>);

impl ResourceLoader for MemLoader {
    fn load(&self, url: &Url) -> Option<Resource> {
        self.0.get(&url.href()).map(|s| Resource {
            bytes: s.as_bytes().to_vec(),
            url: url.clone(),
            content_type: None,
        })
    }
}

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

/// El serializador compacto de los tests de WPT (`adoption-agency-reparenting.js`).
const TREE: &str = "function ser(n) {\
  if (n.nodeType === 3) return '\"' + n.data + '\"';\
  if (n.nodeType !== 1) return '';\
  var label = '<' + n.localName + (n.id ? '#' + n.id : '') + '>';\
  if (n.localName === 'script') return label;\
  var kids = tree(n); return kids ? label + '[' + kids + ']' : label; }\
function tree(node) { return Array.prototype.map.call(node.childNodes, ser).join(''); }";

impl Page {
    /// Una página cargada por el parser de red, con sus scripts.
    fn scripted(body: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/", None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::default()), url.clone()),
        );
        bom::script::install_scripting(
            &mut rt.agent,
            realm,
            &url,
            Arc::new(MemLoader(HashMap::new())),
        );
        let html = format!("<!DOCTYPE html><script>{TREE}</script>{body}");
        bom::script::begin_parse(&mut rt.agent, realm, &url);
        bom::script::feed(&mut rt.agent, realm, &html);
        bom::script::finish(&mut rt.agent, realm);
        bom::script::resources_loaded(&mut rt.agent, realm);
        Page { rt, ctx }
    }

    fn show(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        }
    }
}

#[test]
fn custom_element_is_constructed_before_its_attributes_and_children() {
    let mut p = Page::scripted(
        "<body><script>window.log = [];\
         customElements.define('x-a', class extends HTMLElement {\
           static get observedAttributes() { return ['id']; }\
           constructor() { super(); log.push('ctor attrs=' + this.attributes.length +\
             ' parent=' + (this.parentNode ? this.parentNode.localName : 'none')); }\
           attributeChangedCallback(n, o, v) { log.push('attr ' + n + '=' + v +\
             ' kids=' + this.childNodes.length); }\
           connectedCallback() { log.push('connected kids=' + this.childNodes.length); }\
         })</script><x-a id=a>hola <b>mundo</b></x-a>",
    );
    assert_eq!(
        p.show("log.join('; ')"),
        "ctor attrs=0 parent=none; attr id=a kids=0; connected kids=0"
    );
    assert_eq!(p.show("tree(document.getElementById('a'))"), "\"hola \"<b>[\"mundo\"]");
    assert_eq!(
        p.show("document.getElementById('a') instanceof customElements.get('x-a')"),
        "true"
    );
}

#[test]
fn constructor_that_moves_the_intended_parent_into_its_shadow_root_drops_the_element() {
    // `custom-element-reparenting-shadow-cycle.html`: el elemento ya es un *host-including
    // inclusive ancestor* del sitio donde iba, así que no se inserta, pero sigue en la pila.
    let mut p = Page::scripted(
        "<script>customElements.define('x-a', class extends HTMLElement { constructor() {\
           super(); if (window.el) return; window.el = this; window.b = document.body;\
           window.root = this.attachShadow({mode: 'open'}); root.appendChild(b) } })</script>\
         <body><x-a><p>x</p></x-a>",
    );
    assert_eq!(p.show("String(document.body)"), "null");
    assert_eq!(p.show("String(el.parentNode)"), "null");
    assert_eq!(p.show("root.firstChild === b"), "true");
    assert_eq!(p.show("tree(el)"), "<p>[\"x\"]");
}

#[test]
fn document_write_throws_while_the_parser_constructs_a_custom_element() {
    // El *throw-on-dynamic-markup-insertion counter* (§8.4.1) cubre al constructor.
    let mut p = Page::scripted(
        "<body><script>customElements.define('x-w', class extends HTMLElement { constructor() {\
           super(); try { document.write('<i>no</i>'); window.r = 'escribió' }\
           catch (e) { window.r = e.name } } })</script><x-w id=w></x-w>",
    );
    assert_eq!(p.show("r"), "InvalidStateError");
    assert_eq!(p.show("document.getElementsByTagName('i').length"), "0");
}

#[test]
fn custom_elements_inside_a_template_are_not_constructed() {
    // El contenido de un `template` es de su documento inerte, que no tiene registro.
    let mut p = Page::scripted(
        "<body><script>window.n = 0; customElements.define('x-t', class extends HTMLElement {\
           constructor() { super(); n++ } })</script><template><x-t></x-t></template>",
    );
    assert_eq!(p.show("n"), "0");
}

#[test]
fn inserting_into_a_script_the_page_inserted_runs_it_between_two_insertions() {
    // `foster-parenting-moved-table.window.html`: un solo `<nobr>` hace foster parenting dos
    // veces; la primera inserción cae en el `script` que tiene la tabla y lo ejecuta, y la
    // segunda va adonde quedó la tabla.
    let mut p = Page::scripted(
        "<div id=destination></div><table><nobr><b><tr><script>\
         window.moveTable = () => { const table = document.querySelector('table');\
           window.when = table.parentNode.id; destination.append(table); };\
         const f = document.createElement('script'); f.id = 'foster-parent';\
         f.type = 'unknown/type'; f.append('moveTable()'); document.head.append(f);\
         f.append(document.querySelector('table')); f.type = '';\
         </script><nobr></table>",
    );
    assert_eq!(p.show("when"), "foster-parent");
    let second = "[...document.querySelectorAll('b')].find(b => \
                  b.firstElementChild && b.firstElementChild.localName === 'nobr')";
    assert_eq!(p.show(&format!("{second}.parentNode.id")), "destination");
    assert_eq!(
        p.show(&format!("{second}.nextSibling === document.querySelector('table')")),
        "true"
    );
}

#[test]
fn adoption_agency_does_not_append_the_clone_into_itself() {
    // `custom-elements/append-children-to-new-parent-cycle.html`: la reacción del clon lo
    // mete en el *furthest block* antes de que reciba sus hijos.
    let mut p = Page::scripted(
        "<script>class XBold extends HTMLElement {\
           static get observedAttributes() { return ['x']; }\
           attributeChangedCallback() { const m = document.getElementById('marker');\
             if (!m || window.moved) return; window.moved = true; m.appendChild(this); } }\
         customElements.define('x-bold', XBold, {extends: 'b'});</script>\
         <b is=x-bold x=1><p id=marker></b>",
    );
    assert_eq!(
        p.show(
            "[...document.querySelectorAll('*')].every(n => { let d = 0;\
             for (let a = n; a; a = a.parentNode) if (++d > 1000) return false; return true; })"
        ),
        "true"
    );
    assert_eq!(p.show("moved"), "true");
}

#[test]
fn a_node_adopted_by_a_constructor_keeps_being_built_in_the_other_document() {
    // El emparejado de `dom::adopt` con una frontera de script dentro de un token: el
    // constructor lleva el `div` abierto a otro documento antes de que el parser inserte el
    // custom element en él.
    let mut p = Page::scripted(
        "<body><script>window.other = document.implementation.createHTMLDocument('');\
         customElements.define('x-m', class extends HTMLElement { constructor() { super();\
           window.ca = document.getElementById('ca'); other.body.appendChild(ca) } })</script>\
         <div id=ca>1<x-m>2</x-m>3</div><i>después</i>",
    );
    assert_eq!(p.show("tree(other.body)"), "<div#ca>[\"1\"<x-m>[\"2\"]\"3\"]");
    assert_eq!(p.show("tree(document.body)"), "<script><i>[\"después\"]");
    assert_eq!(
        p.show("ca === other.getElementById('ca') && ca.ownerDocument === other"),
        "true"
    );
}
