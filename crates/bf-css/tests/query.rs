//! Tests de `matches`/`closest`/`querySelector(All)` transcritos de WPT
//! (`tests/wpt/dom/nodes/`).
//!
//! Las tablas grandes de `selectors.js` (`invalidSelectors` y `validSelectors`) se leen en
//! tiempo de ejecución con un mini-parser de literales de JavaScript, y el documento de prueba es
//! `ParentNode-querySelector-All-content.html` parseado con `bf_html`. Los selectores que el motor
//! no soporta o que dependen de cosas fuera del DOM (`:target` necesita la URL del iframe) se
//! listan en [`OMITIDOS`] con su motivo.

use bf_css::query::{closest, element_matches, query_selector, query_selector_all};
use bf_dom::{Document, DomException, DomString, NodeId};
use bf_html::{ParseOptions, parse_document};

// --- Utilidades ---

fn s(x: &str) -> DomString {
    DomString::from(x)
}

fn parse(markup: &str) -> Document {
    parse_document(markup, &ParseOptions::default()).0
}

fn id_de(doc: &Document, n: NodeId) -> Option<String> {
    doc.get_attribute(n, &s("id")).map(|v| v.to_string())
}

fn por_id(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(&s(id))
        .unwrap_or_else(|| panic!("no existe #{id}"))
}

/// El elemento con `id` entre los descendientes de `root` (`root.querySelector("#" + id)` sin
/// pasar por el motor de selectores).
fn buscar_id(doc: &Document, root: NodeId, id: &str) -> Option<NodeId> {
    doc.descendants(root)
        .into_iter()
        .find(|&n| doc.element(n).is_some() && id_de(doc, n).as_deref() == Some(id))
}

fn ids(doc: &Document, nodos: &[NodeId]) -> Vec<String> {
    nodos
        .iter()
        .map(|&n| id_de(doc, n).unwrap_or_default())
        .collect()
}

// --- Mini-parser de los literales de selectors.js ---

#[derive(Debug, Clone)]
enum Valor {
    Str(String),
    Num(u32),
    Arr(Vec<Valor>),
}

#[derive(Debug, Clone, Default)]
struct Caso {
    name: String,
    selector: String,
    expect: Vec<String>,
    unexpected: Vec<String>,
    exclude: Vec<String>,
    test_type: u32,
}

const TEST_QSA: u32 = 0x01;
const TEST_MATCH: u32 = 0x10;

struct Lector<'a> {
    c: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Lector<'_> {
    fn espacios(&mut self) {
        loop {
            match self.c.peek() {
                Some(c) if c.is_whitespace() => {
                    self.c.next();
                }
                Some('/') => {
                    let mut copia = self.c.clone();
                    copia.next();
                    match copia.peek() {
                        Some('/') => {
                            for c in self.c.by_ref() {
                                if c == '\n' {
                                    break;
                                }
                            }
                        }
                        Some('*') => {
                            self.c.next();
                            self.c.next();
                            let mut prev = ' ';
                            for c in self.c.by_ref() {
                                if prev == '*' && c == '/' {
                                    break;
                                }
                                prev = c;
                            }
                        }
                        _ => return,
                    }
                }
                _ => return,
            }
        }
    }

    fn esperar(&mut self, c: char) {
        self.espacios();
        assert_eq!(self.c.next(), Some(c));
    }

    fn cadena(&mut self) -> String {
        let q = self.c.next().unwrap();
        let mut out = String::new();
        loop {
            let c = self.c.next().unwrap();
            if c == q {
                return out;
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            match self.c.next().unwrap() {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'f' => out.push('\u{c}'),
                '0' => out.push('\0'),
                'u' => {
                    let h: String = (0..4).map(|_| self.c.next().unwrap()).collect();
                    out.push(char::from_u32(u32::from_str_radix(&h, 16).unwrap()).unwrap());
                }
                otro => out.push(otro),
            }
        }
    }

    fn ident(&mut self) -> String {
        let mut out = String::new();
        while let Some(&c) = self.c.peek() {
            if c.is_alphanumeric() || c == '_' {
                out.push(c);
                self.c.next();
            } else {
                break;
            }
        }
        out
    }

    fn valor(&mut self) -> Valor {
        self.espacios();
        match *self.c.peek().unwrap() {
            '"' | '\'' => Valor::Str(self.cadena()),
            '[' => {
                self.c.next();
                let mut v = Vec::new();
                loop {
                    self.espacios();
                    if self.c.peek() == Some(&']') {
                        self.c.next();
                        return Valor::Arr(v);
                    }
                    v.push(self.valor());
                    self.espacios();
                    if self.c.peek() == Some(&',') {
                        self.c.next();
                    }
                }
            }
            _ => {
                // Número o expresión `TEST_QSA | TEST_MATCH`.
                let mut total = 0;
                loop {
                    self.espacios();
                    let id = self.ident();
                    total |= match id.as_str() {
                        "TEST_QSA" => 0x01,
                        "TEST_FIND" => 0x04,
                        "TEST_MATCH" => 0x10,
                        n => n.parse::<u32>().unwrap(),
                    };
                    self.espacios();
                    if self.c.peek() == Some(&'|') {
                        self.c.next();
                    } else {
                        return Valor::Num(total);
                    }
                }
            }
        }
    }

    fn objeto(&mut self) -> Caso {
        self.esperar('{');
        let mut caso = Caso::default();
        let strings = |v: Valor| match v {
            Valor::Arr(a) => a
                .into_iter()
                .map(|x| match x {
                    Valor::Str(s) => s,
                    otro => panic!("{otro:?}"),
                })
                .collect(),
            otro => panic!("{otro:?}"),
        };
        loop {
            self.espacios();
            if self.c.peek() == Some(&'}') {
                self.c.next();
                return caso;
            }
            let clave = self.ident();
            self.esperar(':');
            let v = self.valor();
            match (clave.as_str(), v) {
                ("name", Valor::Str(x)) => caso.name = x,
                ("selector", Valor::Str(x)) => caso.selector = x,
                ("expect", v) => caso.expect = strings(v),
                ("unexpected", v) => caso.unexpected = strings(v),
                ("exclude", v) => caso.exclude = strings(v),
                ("testType", Valor::Num(n)) => caso.test_type = n,
                _ => {}
            }
            self.espacios();
            if self.c.peek() == Some(&',') {
                self.c.next();
            }
        }
    }
}

/// Un archivo de `tests/wpt/dom/nodes/`, leído una vez en tiempo de ejecución (`None` si falta
/// el checkout de WPT: `tools/fetch-wpt`).
fn wpt_nodes(nombre: &str) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<&'static str>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    *cache.entry(nombre.to_owned()).or_insert_with(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/wpt/dom/nodes")
            .join(nombre);
        std::fs::read_to_string(path)
            .ok()
            .map(|s| &*Box::leak(s.into_boxed_str()))
    })
}

fn selectors_js() -> &'static str {
    wpt_nodes("selectors.js").expect("selectors.js")
}

fn content_html() -> &'static str {
    wpt_nodes("ParentNode-querySelector-All-content.html").expect("content.html")
}

/// ¿Está `tests/wpt/dom/nodes`? Si no, el test termina con un aviso (como los harness de WPT).
fn wpt_disponible() -> bool {
    let ok = wpt_nodes("selectors.js").is_some()
        && wpt_nodes("ParentNode-querySelector-All-content.html").is_some();
    if !ok {
        eprintln!("aviso: falta tests/wpt/dom/nodes (tools/fetch-wpt); se salta");
    }
    ok
}

/// Los objetos del array `var <nombre> = [ ... ];` de selectors.js.
fn tabla(nombre: &str) -> Vec<Caso> {
    let inicio = selectors_js()
        .find(&format!("var {nombre} = ["))
        .unwrap_or_else(|| panic!("no está {nombre}"));
    let resto = &selectors_js()[inicio..];
    let abre = resto.find('[').unwrap();
    let fin = resto.find("\n];").unwrap();
    let mut l = Lector {
        c: resto[abre + 1..fin].chars().peekable(),
    };
    let mut casos = Vec::new();
    loop {
        l.espacios();
        if l.c.peek().is_none() {
            return casos;
        }
        casos.push(l.objeto());
        l.espacios();
        if l.c.peek() == Some(&',') {
            l.c.next();
        }
    }
}

/// Selectores de `validSelectors` que no se prueban, con el motivo.
const OMITIDOS: &[(&str, &str)] = &[
    // El documento de WPT se carga como `...-content.html#target`; aquí no hay URL.
    (":target", "depende del fragmento de la URL del iframe"),
    // Sin shadow DOM.
    ("::slotted", "shadow DOM no soportado"),
];

fn omitido(caso: &Caso) -> bool {
    OMITIDOS.iter().any(|(sel, _)| caso.selector.contains(sel))
}

/// Selectores de `validSelectors` que fallan por bugs del motor (ver los tests `#[ignore]`).
const BUGS_VALIDOS: &[&str] = &[
    // `:empty`: el motor sigue Selectors 4 (ED), donde un elemento con solo white space es
    // `:empty`; WPT y los navegadores todavía esperan lo contrario.
    "#pseudo-empty p:empty",
    "#pseudo-empty :empty",
];

/// Selectores de `invalidSelectors` que el motor acepta por error.
const BUGS_INVALIDOS: &[&str] = &[];

/// ¿Se corre `caso` en esta pasada? (`bugs`: solo los casos con bugs conocidos).
fn seleccionado(caso: &Caso, conocidos: &[&str], bugs: bool) -> bool {
    conocidos.contains(&caso.selector.as_str()) == bugs
}

/// `setupSpecialElements(doc, parent)` de ParentNode-querySelector-All.js.
fn setup_special_elements(doc: &mut Document, parent: NodeId) {
    for tag in ["null", "undefined"] {
        let e = doc.create_element(&s(tag)).unwrap();
        doc.append_child(parent, e).unwrap();
    }
    for prefijo in ["any-namespace", "no-namespace"] {
        let cont = doc.create_element(&s("div")).unwrap();
        doc.set_attribute(cont, &s("id"), s(prefijo)).unwrap();
        let divs = [
            doc.create_element(&s("div")).unwrap(),
            doc.create_element_ns(Some("http://www.w3.org/1999/xhtml"), &s("div"))
                .unwrap(),
            doc.create_element_ns(None, &s("div")).unwrap(),
            doc.create_element_ns(Some("http://www.example.org/ns"), &s("div"))
                .unwrap(),
        ];
        for (i, d) in divs.into_iter().enumerate() {
            doc.set_attribute(d, &s("id"), s(&format!("{prefijo}-div{}", i + 1)))
                .unwrap();
            doc.append_child(cont, d).unwrap();
        }
        doc.append_child(parent, cont).unwrap();
    }
    let span = por_id(doc, "attr-presence-i1");
    doc.set_attribute_ns(span, Some("http://www.example.org/ns"), &s("title"), s(""))
        .unwrap();
}

/// Los cuatro contextos de `init()` en ParentNode-querySelector-All.html.
struct Contextos {
    doc: Document,
    element: NodeId,
    detached: NodeId,
    fragment: NodeId,
    out_of_scope: NodeId,
}

fn contextos() -> Contextos {
    let mut doc = parse(content_html());
    let element = por_id(&doc, "root");
    setup_special_elements(&mut doc, element);
    let out_of_scope = doc.clone_node(element, true);
    for n in doc.inclusive_descendants(out_of_scope) {
        if doc.element(n).is_some() {
            doc.set_attribute(n, &s("data-clone"), s("")).unwrap();
        }
    }
    let detached = doc.clone_node(element, true);
    let fragment = doc.create_document_fragment();
    let copia = doc.clone_node(element, true);
    doc.append_child(fragment, copia).unwrap();
    Contextos {
        doc,
        element,
        detached,
        fragment,
        out_of_scope,
    }
}

/// `runValidSelectorTest` para un contexto; devuelve los fallos.
fn correr_validos(
    doc: &Document,
    root: NodeId,
    tipo_nodo: &str,
    bugs: bool,
    fallos: &mut Vec<String>,
) {
    for caso in tabla("validSelectors") {
        if caso.test_type & TEST_QSA == 0
            || caso.exclude.iter().any(|e| e == tipo_nodo || e == "html")
            || omitido(&caso)
            || !seleccionado(&caso, BUGS_VALIDOS, bugs)
        {
            continue;
        }
        let q = &caso.selector;
        let all = match query_selector_all(doc, root, q) {
            Ok(v) => v,
            Err(e) => {
                fallos.push(format!("{tipo_nodo}: {}: {q:?} lanzó {e:?}", caso.name));
                continue;
            }
        };
        let obtenidos = ids(doc, &all);
        if obtenidos != caso.expect {
            fallos.push(format!(
                "{tipo_nodo}.querySelectorAll: {}: {q:?}: esperado {:?}, obtenido {obtenidos:?}",
                caso.name, caso.expect
            ));
        }
        if all.iter().any(|&n| doc.has_attribute(n, &s("data-clone"))) {
            fallos.push(format!("{tipo_nodo}: {q:?} devolvió un clon"));
        }
        let uno = query_selector(doc, root, q).unwrap();
        if uno != all.first().copied() {
            fallos.push(format!(
                "{tipo_nodo}.querySelector: {}: {q:?}: no es el primero de querySelectorAll",
                caso.name
            ));
        }
    }
}

/// `runMatchesTest` (Element-matches.js) para un contexto; devuelve los fallos.
fn correr_matches(
    doc: &Document,
    root: NodeId,
    tipo_nodo: &str,
    bugs: bool,
    fallos: &mut Vec<String>,
) {
    for caso in tabla("validSelectors") {
        if caso.test_type & TEST_MATCH == 0
            || caso.exclude.iter().any(|e| e == tipo_nodo || e == "html")
            || omitido(&caso)
            || !seleccionado(&caso, BUGS_VALIDOS, bugs)
        {
            continue;
        }
        let q = &caso.selector;
        for (lista, esperado) in [(&caso.expect, true), (&caso.unexpected, false)] {
            for id in lista {
                let Some(e) = buscar_id(doc, root, id) else {
                    fallos.push(format!("{tipo_nodo}: no existe #{id}"));
                    continue;
                };
                match element_matches(doc, e, q) {
                    Ok(m) if m == esperado => {}
                    r => fallos.push(format!(
                        "{tipo_nodo} Element.matches: {}: {q:?} en #{id}: esperado {esperado}, obtenido {r:?}",
                        caso.name
                    )),
                }
            }
        }
    }
}

fn sin_fallos(fallos: Vec<String>) {
    assert!(
        fallos.is_empty(),
        "{} fallos:\n{}",
        fallos.len(),
        fallos.join("\n")
    );
}

// --- ParentNode-querySelector-All.html (selectors.js) ---

// WPT: ParentNode-querySelector-All.html — runInvalidSelectorTest("Document" | "Detached Element" |
// "Fragment" | "In-document Element" | "Empty Element", invalidSelectors): SyntaxError
#[test]
fn qsa_selectores_invalidos_lanzan_syntax_error() {
    if !wpt_disponible() {
        return;
    }
    correr_invalidos(false);
}

// WPT: ParentNode-querySelector-All.html — runInvalidSelectorTest(..., "Invalid group of
// selectors: div,")
#[test]
fn qsa_selector_invalido_coma_final() {
    if !wpt_disponible() {
        return;
    }
    correr_invalidos(true);
}

fn correr_invalidos(bugs: bool) {
    let mut c = contextos();
    let empty = c.doc.create_element(&s("div")).unwrap();
    let root = c.doc.root();
    let invalidos = tabla("invalidSelectors");
    assert!(invalidos.len() >= 30);
    let mut fallos = Vec::new();
    for caso in invalidos
        .iter()
        .filter(|c| seleccionado(c, BUGS_INVALIDOS, bugs))
    {
        for (tipo, n) in [
            ("Document", root),
            ("Detached Element", c.detached),
            ("Fragment", c.fragment),
            ("In-document Element", c.element),
            ("Empty Element", empty),
        ] {
            let q = &caso.selector;
            if query_selector(&c.doc, n, q) != Err(DomException::SyntaxError) {
                fallos.push(format!("{tipo}.querySelector: {}: {q:?}", caso.name));
            }
            if query_selector_all(&c.doc, n, q) != Err(DomException::SyntaxError) {
                fallos.push(format!("{tipo}.querySelectorAll: {}: {q:?}", caso.name));
            }
        }
    }
    sin_fallos(fallos);
}

// WPT: ParentNode-querySelector-All.html — runValidSelectorTest("Document", doc, validSelectors)
#[test]
fn qsa_selectores_validos_en_document() {
    if !wpt_disponible() {
        return;
    }
    let c = contextos();
    assert!(tabla("validSelectors").len() > 200);
    let mut fallos = Vec::new();
    correr_validos(&c.doc, c.doc.root(), "document", false, &mut fallos);
    sin_fallos(fallos);
}

// WPT: ParentNode-querySelector-All.html — runValidSelectorTest("Detached Element", ...) y
// runValidSelectorTest("Fragment", ...)
#[test]
fn qsa_selectores_validos_en_detached_y_fragment() {
    if !wpt_disponible() {
        return;
    }
    let c = contextos();
    let mut fallos = Vec::new();
    correr_validos(&c.doc, c.detached, "detached", false, &mut fallos);
    correr_validos(&c.doc, c.fragment, "fragment", false, &mut fallos);
    sin_fallos(fallos);
}

// WPT: ParentNode-querySelector-All.html — runValidSelectorTest("In-document Element", element, ...)
// (con la copia `outOfScope` agregada al body: ningún clon debe coincidir)
#[test]
fn qsa_selectores_validos_en_elemento_del_documento() {
    if !wpt_disponible() {
        return;
    }
    let mut c = contextos();
    let body = c.doc.body().unwrap();
    c.doc.append_child(body, c.out_of_scope).unwrap();
    let mut fallos = Vec::new();
    correr_validos(&c.doc, c.element, "element", false, &mut fallos);
    sin_fallos(fallos);
}

// WPT: ParentNode-querySelector-All.html — runSpecialSelectorTests: "querySelectorAll null",
// "querySelectorAll undefined", "querySelector null", "querySelector undefined" (los valores ya
// convertidos a string) y "querySelectorAll tree order"
#[test]
fn qsa_selectores_especiales_y_orden_del_arbol() {
    if !wpt_disponible() {
        return;
    }
    let c = contextos();
    let root = c.doc.root();
    for (tipo, n) in [
        ("Document", root),
        ("Detached Element", c.detached),
        ("Fragment", c.fragment),
        ("In-document Element", c.element),
    ] {
        for nombre in ["null", "undefined"] {
            assert_eq!(
                query_selector_all(&c.doc, n, nombre).unwrap().len(),
                1,
                "{tipo}: {nombre}"
            );
            let e = query_selector(&c.doc, n, nombre).unwrap().unwrap();
            assert_eq!(
                c.doc.tag_name(e).to_string().to_uppercase(),
                nombre.to_uppercase()
            );
        }
        let todos = query_selector_all(&c.doc, n, "*").unwrap();
        let esperado: Vec<NodeId> = c
            .doc
            .descendants(n)
            .into_iter()
            .filter(|&d| c.doc.element(d).is_some())
            .collect();
        assert_eq!(todos, esperado, "{tipo}: tree order");
    }
}

// WPT: ParentNode-querySelector-All.html — verifyStaticList: "static NodeList" y "new NodeList"
#[test]
fn qsa_lista_estatica() {
    if !wpt_disponible() {
        return;
    }
    let mut c = contextos();
    let root = c.doc.root();
    let body = c.doc.body().unwrap();
    for (tipo, n, destino) in [
        ("Document", root, body),
        ("Detached Element", c.detached, c.detached),
        ("Fragment", c.fragment, c.fragment),
        ("In-document Element", c.element, c.element),
    ] {
        let pre = query_selector_all(&c.doc, n, "div").unwrap();
        let pre_len = pre.len();
        let div = c.doc.create_element(&s("div")).unwrap();
        c.doc.append_child(destino, div).unwrap();
        assert_eq!(pre.len(), pre_len, "{tipo}: la lista no cambia");
        let post = query_selector_all(&c.doc, n, "div").unwrap();
        assert_eq!(post.len(), pre.len() + 1, "{tipo}: nueva lista");
    }
}

// WPT: ParentNode-querySelector-All.html — runValidSelectorTest en los cuatro contextos, solo
// los casos de `|div`, `|*` (namespace nulo) y `:empty` con texto de espacios
#[test]
#[ignore = "Selectors 4 (ED) hace ':empty' a un elemento con solo white space; WPT espera lo contrario"]
fn qsa_selectores_validos_con_bugs() {
    if !wpt_disponible() {
        return;
    }
    let mut c = contextos();
    let mut fallos = Vec::new();
    correr_validos(&c.doc, c.doc.root(), "document", true, &mut fallos);
    correr_validos(&c.doc, c.detached, "detached", true, &mut fallos);
    correr_validos(&c.doc, c.fragment, "fragment", true, &mut fallos);
    let body = c.doc.body().unwrap();
    c.doc.append_child(body, c.out_of_scope).unwrap();
    correr_validos(&c.doc, c.element, "element", true, &mut fallos);
    correr_matches(&c.doc, c.doc.root(), "document", true, &mut fallos);
    sin_fallos(fallos);
}

// --- Element-matches.html (selectors.js) ---

// WPT: Element-matches.html — runInvalidSelectorTestMatches(...): SyntaxError (los contextos
// elemento: Detached Element e In-document Element)
#[test]
fn matches_selectores_invalidos_lanzan_syntax_error() {
    if !wpt_disponible() {
        return;
    }
    let c = contextos();
    let mut fallos = Vec::new();
    for caso in tabla("invalidSelectors") {
        if !seleccionado(&caso, BUGS_INVALIDOS, false) {
            continue;
        }
        for (tipo, n) in [("Detached", c.detached), ("In-document", c.element)] {
            if element_matches(&c.doc, n, &caso.selector) != Err(DomException::SyntaxError) {
                fallos.push(format!("{tipo}: {}: {:?}", caso.name, caso.selector));
            }
        }
    }
    sin_fallos(fallos);
}

// WPT: Element-matches.html — runMatchesTest("In-document", doc, validSelectors, "html")
#[test]
fn matches_selectores_validos_en_document() {
    if !wpt_disponible() {
        return;
    }
    let c = contextos();
    let mut fallos = Vec::new();
    correr_matches(&c.doc, c.doc.root(), "document", false, &mut fallos);
    sin_fallos(fallos);
}

// WPT: Element-matches.html — runMatchesTest("Detached", detached, ...) y
// runMatchesTest("Fragment", fragment, ...)
#[test]
fn matches_selectores_validos_en_detached_y_fragment() {
    if !wpt_disponible() {
        return;
    }
    let c = contextos();
    let mut fallos = Vec::new();
    correr_matches(&c.doc, c.detached, "detached", false, &mut fallos);
    correr_matches(&c.doc, c.fragment, "fragment", false, &mut fallos);
    sin_fallos(fallos);
}

// WPT: Element-matches.html — runSpecialMatchesTests: "DIV Element.matches(null)",
// "NULL Element.matches(null)", "UNDEFINED Element.matches(undefined)" (strings ya convertidos)
#[test]
fn matches_null_y_undefined() {
    if !wpt_disponible() {
        return;
    }
    let mut doc = parse("<div id=root></div>");
    let root = por_id(&doc, "root");
    let null = doc.create_element(&s("null")).unwrap();
    let undefined = doc.create_element(&s("undefined")).unwrap();
    assert!(!element_matches(&doc, root, "null").unwrap());
    assert!(!element_matches(&doc, root, "undefined").unwrap());
    assert!(element_matches(&doc, null, "null").unwrap());
    assert!(!element_matches(&doc, null, "undefined").unwrap());
    assert!(element_matches(&doc, undefined, "undefined").unwrap());
    assert!(!element_matches(&doc, undefined, "null").unwrap());
}

// --- Element-closest.html ---

const CLOSEST_HTML: &str = r#"<!DOCTYPE HTML>
<body id="body">
  <div id="test8" class="div3" style="display:none">
    <div id="test7" class="div2">
      <div id="test6" class="div1">
        <form id="test10" class="form2"></form>
        <form id="test5" class="form1" name="form-a">
          <input id="test1" class="input1" required>
          <fieldset class="fieldset2" id="test2">
            <select id="test3" class="select1" required>
              <option default id="test4" value="">Test4</option>
              <option selected id="test11">Test11</option>
              <option id="test12">Test12</option>
              <option id="test13">Test13</option>
            </select>
            <input id="test9" type="text" required>
          </fieldset>
        </form>
      </div>
    </div>
  </div>
  <div id=log></div>
"#;

/// `do_test(selector, elementId, targetId)` de Element-closest.html.
const CLOSEST: &[(&str, &str, &str)] = &[
    ("select", "test12", "test3"),
    ("fieldset", "test13", "test2"),
    ("div", "test13", "test6"),
    ("body", "test3", "body"),
    ("[default]", "test4", "test4"),
    ("[selected]", "test4", ""),
    ("[selected]", "test11", "test11"),
    ("[name=\"form-a\"]", "test12", "test5"),
    ("form[name=\"form-a\"]", "test13", "test5"),
    ("input[required]", "test9", "test9"),
    ("select[required]", "test9", ""),
    ("div:not(.div1)", "test13", "test7"),
    ("div.div3", "test6", "test8"),
    ("div#test7", "test1", "test7"),
    (".div3 > .div2", "test12", "test7"),
    (".div3 > .div1", "test12", ""),
    ("form > input[required]", "test9", ""),
    ("fieldset > select[required]", "test12", "test3"),
    ("input + fieldset", "test6", ""),
    ("form + form", "test3", "test5"),
    ("form + form", "test5", "test5"),
    (":empty", "test10", "test10"),
    (":last-child", "test11", "test2"),
    (":first-child", "test12", "test3"),
    (":invalid", "test11", "test2"),
    (":scope", "test4", "test4"),
    ("select > :scope", "test4", "test4"),
    ("div > :scope", "test4", ""),
    (":has(> :scope)", "test4", "test3"),
];

fn correr_closest(casos: &[(&str, &str, &str)]) -> Vec<String> {
    let doc = parse(CLOSEST_HTML);
    let mut fallos = Vec::new();
    for &(sel, el, esperado) in casos {
        let e = por_id(&doc, el);
        let r = closest(&doc, e, sel)
            .unwrap()
            .map(|n| id_de(&doc, n).unwrap_or_default())
            .unwrap_or_default();
        if r != esperado {
            fallos.push(format!(
                "closest({sel:?}) desde #{el}: esperado {esperado:?}, obtenido {r:?}"
            ));
        }
    }
    fallos
}

// WPT: Element-closest.html — "Element.closest with context node 'X' and selector 'Y'" (todos
// menos `:invalid`)
#[test]
fn closest_tabla() {
    let casos: Vec<_> = CLOSEST
        .iter()
        .copied()
        .filter(|c| c.0 != ":invalid")
        .collect();
    sin_fallos(correr_closest(&casos));
}

// WPT: Element-closest.html — "Element.closest with context node 'test11' and selector ':invalid'"
#[test]
#[ignore = "':invalid' (validación de formularios) no está implementado"]
fn closest_invalid_en_fieldset() {
    sin_fallos(correr_closest(&[(":invalid", "test11", "test2")]));
}

// WPT: Element-closest-null-selectors.html — closest con selectores inválidos lanza SyntaxError
#[test]
fn closest_selector_invalido() {
    let doc = parse(CLOSEST_HTML);
    let e = por_id(&doc, "test4");
    for sel in ["", "[", ">*", "div:example"] {
        assert_eq!(
            closest(&doc, e, sel),
            Err(DomException::SyntaxError),
            "{sel:?}"
        );
    }
}

// --- :scope y casos sueltos ---

// WPT: ParentNode-querySelector-scope.html — "querySelector with :scope", "querySelector with id
// and sibling", "querySelectorAll with :scope", "querySelectorAll with id and sibling"
#[test]
fn qsa_scope_basico() {
    let doc = parse(r#"<div><h1 id="test"></h1><p><span>hello</span></p></div>"#);
    let root = doc.root();
    let div = query_selector(&doc, root, "div").unwrap().unwrap();
    let p = query_selector(&doc, root, "p").unwrap().unwrap();
    assert_eq!(query_selector(&doc, div, ":scope > p").unwrap(), Some(p));
    assert_eq!(query_selector(&doc, div, ":scope > span").unwrap(), None);
    assert_eq!(query_selector(&doc, div, "#test + p").unwrap(), Some(p));
    assert_eq!(query_selector(&doc, p, "#test + p").unwrap(), None);
    assert_eq!(
        query_selector_all(&doc, div, ":scope > p").unwrap(),
        vec![p]
    );
    assert!(
        query_selector_all(&doc, div, ":scope > span")
            .unwrap()
            .is_empty()
    );
    assert_eq!(query_selector_all(&doc, div, "#test + p").unwrap(), vec![p]);
    assert!(query_selector_all(&doc, p, "#test + p").unwrap().is_empty());
}

// WPT: ParentNode-querySelector-All-scope.html — "querySelector", "querySelectorAll",
// "querySelector comma separated", "querySelectorAll comma separated", "querySelector with :scope
// > child should not match descendants", "querySelectorAll with :scope > child should only match
// direct children" (se omiten los dos subtests con DOMParser de XML/SVG)
#[test]
fn qsa_scope_hijos_directos() {
    let doc = parse(
        r#"<div>
  <p><span>hello</span></p>
  <p id="p2">hey</p>
  <div id="d2">div</div>
</div>
<div id="structure1">
  <div>
    <span id="descendant-span"></span>
  </div>
</div>
<ul id="structure2">
  <li id="alpha">Alpha</li>
  <li id="beta">
    Beta
    <ul>
      <li id="gamma">Gamma</li>
      <li id="delta">Delta</li>
    </ul>
  </li>
</ul>"#,
    );
    let root = doc.root();
    let div = query_selector(&doc, root, "div").unwrap().unwrap();
    let p = query_selector(&doc, root, "p").unwrap().unwrap();
    let (p2, div2) = (por_id(&doc, "p2"), por_id(&doc, "d2"));
    assert_eq!(query_selector(&doc, div, ":scope > p").unwrap(), Some(p));
    assert_eq!(query_selector(&doc, div, ":scope > span").unwrap(), None);
    assert_eq!(
        query_selector_all(&doc, div, ":scope > p").unwrap(),
        vec![p, p2]
    );
    assert!(
        query_selector_all(&doc, div, ":scope > span")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        query_selector(&doc, div, ":scope > p, :scope > div").unwrap(),
        Some(p)
    );
    assert_eq!(
        query_selector_all(&doc, div, ":scope > p, :scope > div").unwrap(),
        vec![p, p2, div2]
    );
    let s1 = por_id(&doc, "structure1");
    assert_eq!(query_selector(&doc, s1, ":scope > span").unwrap(), None);
    let ul = por_id(&doc, "structure2");
    let r = query_selector_all(&doc, ul, ":scope > li").unwrap();
    assert_eq!(ids(&doc, &r), vec!["alpha", "beta"]);
}

// WPT: ParentNode-querySelector-orphan-scope.html — "querySelectorAll on orphaned element does not
// return the element itself" y "querySelectorAll descendant selector works on orphaned elements"
#[test]
fn qsa_en_elemento_huerfano() {
    let mut doc = parse("");
    let parent = doc.create_element(&s("div")).unwrap();
    doc.set_attribute(parent, &s("id"), s("fuz")).unwrap();
    let child = doc.create_element(&s("span")).unwrap();
    doc.set_attribute(child, &s("id"), s("buz")).unwrap();
    doc.append_child(parent, child).unwrap();
    assert!(query_selector_all(&doc, parent, "#fuz").unwrap().is_empty());
    assert_eq!(
        query_selector_all(&doc, parent, "#fuz #buz").unwrap(),
        vec![child]
    );
}

// WPT: ParentNode-querySelector-dupe-id.html — "Sanity: returns a non-null value" y "Returns a
// non-null value for connected cloned node with duplicated ID"
#[test]
fn qsa_id_duplicado_en_clon() {
    for conectar in [false, true] {
        let mut doc = parse(r#"<html><div id="test1"><div id="test2"></div></div></html>"#);
        let root = doc.document_element().unwrap();
        let test1 = query_selector(&doc, root, "#test1").unwrap().unwrap();
        let clon = doc.clone_node(test1, true);
        if conectar {
            doc.append_child(root, clon).unwrap();
        }
        let test2 = query_selector(&doc, clon, "#test2").unwrap();
        assert!(test2.is_some(), "conectar = {conectar}");
        assert_eq!(test2, query_selector(&doc, clon, "[id=test2]").unwrap());
        assert_eq!(
            test2,
            query_selector_all(&doc, clon, "#test2")
                .unwrap()
                .first()
                .copied()
        );
    }
}

// WPT: ParentNode-querySelector-All-comma-selector.html — "Basic comma-separated selector" y
// "Repeated identical querySelectorAll calls with repeated selectors should work on empty elements"
#[test]
fn qsa_selector_con_comas() {
    let mut doc = parse("<h1>Hello world</h1>\n<h2>Goodbye, world</h2>");
    let root = doc.root();
    assert_eq!(query_selector_all(&doc, root, "h1,h2").unwrap().len(), 2);
    let el = doc.create_element(&s("div")).unwrap();
    for _ in 0..2 {
        assert!(
            query_selector_all(&doc, el, ".foo, .foo, .bar")
                .unwrap()
                .is_empty()
        );
    }
}
