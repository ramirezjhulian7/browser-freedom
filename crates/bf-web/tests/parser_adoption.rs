//! E1: el parser sigue a los nodos que tiene abiertos cuando un script los lleva a otro
//! documento (HTML §13.2.6.1 y el adoption agency, §13.2.6.4.7; DOM §4.5 *adopt*).

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
        let html = format!("<!DOCTYPE html><script>{TREE}</script><body>{body}");
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

const NEW_DOC: &str = "window.other = document.implementation.createHTMLDocument('');";

#[test]
fn furthest_block_comes_back_with_the_adoption_agency() {
    let mut p = Page::scripted(&format!(
        "<b>1<p id=fb>2<script>{NEW_DOC}window.fbRef = document.getElementById('fb');\
         other.body.appendChild(fbRef)</script></b>3</p>"
    ));
    assert_eq!(
        p.show("tree(document.body)"),
        "<b>[\"1\"]<p#fb>[<b>[\"2\"<script>]\"3\"]"
    );
    assert_eq!(p.show("tree(other.body)"), "");
    assert_eq!(
        p.show("fbRef === document.getElementById('fb') && fbRef.ownerDocument === document"),
        "true"
    );
}

#[test]
fn common_ancestor_keeps_being_built_in_the_other_document() {
    let mut p = Page::scripted(&format!(
        "<div id=ca><b>1<p id=fb>2<script>{NEW_DOC}window.caRef = document.getElementById('ca');\
         other.body.appendChild(caRef)</script></b>3</p></div><i>después</i>"
    ));
    assert_eq!(p.show("tree(document.body)"), "<i>[\"después\"]");
    assert_eq!(
        p.show("tree(other.body)"),
        "<div#ca>[<b>[\"1\"]<p#fb>[<b>[\"2\"<script>]\"3\"]]"
    );
    assert_eq!(
        p.show("caRef === other.getElementById('ca') && caRef.ownerDocument === other"),
        "true"
    );
}

#[test]
fn script_changes_to_the_moved_subtree_reach_the_parser() {
    // El script mueve el subárbol y le añade un hijo al nodo vivo: el parser sigue después de
    // ese hijo. Un `script` que el parser crea dentro del subárbol movido no corre: su node
    // document es el otro documento y no su parser document (HTML §4.12.1.1, "prepare").
    let mut p = Page::scripted(&format!(
        "<div id=ca><p id=p>1<script>{NEW_DOC}other.body.appendChild(document.getElementById('ca'));\
         other.getElementById('p').appendChild(other.createElement('span'))</script>2\
         <script>window.ran = true</script>3</p></div>"
    ));
    assert_eq!(
        p.show("tree(other.getElementById('p'))"),
        "\"1\"<script><span>\"2\"<script>\"3\""
    );
    assert_eq!(p.show("typeof ran"), "undefined");
    assert_eq!(p.show("tree(document.body)"), "");
}

#[test]
fn foster_parenting_into_a_document_with_an_element_child_drops_the_node() {
    let mut p = Page::scripted(&format!(
        "<table><tr><script>{NEW_DOC}other.documentElement.remove();\
         other.appendChild(document.querySelector('table'))</script><b id=fostered>x</b>"
    ));
    assert_eq!(
        p.show("[other.childElementCount, !!other.getElementById('fostered'), !!document.getElementById('fostered')].join()"),
        "1,false,false"
    );
    assert_eq!(p.show("other.documentElement.localName"), "table");
}

#[test]
fn foster_parenting_follows_a_table_moved_into_another_documents_body() {
    // La tabla está en el `body` de otro documento: lo que se hace foster parenting va ahí,
    // delante de ella.
    let mut p = Page::scripted(&format!(
        "<table><tr><script>{NEW_DOC}other.body.appendChild(document.querySelector('table'))\
         </script><b id=fostered>x</b></table>"
    ));
    assert_eq!(
        p.show("tree(other.body)"),
        "<b#fostered>[\"x\"]<table>[<tbody>[<tr>[<script>]]]"
    );
    assert_eq!(p.show("tree(document.body)"), "");
}
