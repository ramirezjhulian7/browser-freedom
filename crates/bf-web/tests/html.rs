//! Las interfaces `HTML*Element` (`bf_web::html`): la clase por elemento, la reflexión de
//! atributos (HTML §2.6.1) y los algoritmos propios de formularios, tablas e hipervínculos.

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    fn new(html: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (mut doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/dir/page.html", None).expect("url");
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
        match self.rt.eval_script(realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => format!("Error: {}", e.message),
        }
    }
}

const DOC: &str = "<!DOCTYPE html><html><head><title>T</title></head><body>\
                   <div id=d></div><a id=a href='/x/y?q=1#h'>enlace</a>\
                   <form id=f action=''><input id=i name=n value=v><select id=s>\
                   <option>uno</option><option value=2 selected>dos</option></select></form>\
                   <table id=t><tr><td>a</td><td>b</td></tr></table></body></html>";

#[test]
fn element_interfaces() {
    let mut p = Page::new(DOC);
    let names = p.show(
        "['div','a','input','table','tr','td','h3','article','foo','blink']\
         .map(n => Object.prototype.toString.call(document.createElement(n))).join()",
    );
    assert_eq!(
        names,
        "[object HTMLDivElement],[object HTMLAnchorElement],[object HTMLInputElement],\
         [object HTMLTableElement],[object HTMLTableRowElement],[object HTMLTableCellElement],\
         [object HTMLHeadingElement],[object HTMLElement],[object HTMLUnknownElement],\
         [object HTMLUnknownElement]"
    );
    assert_eq!(
        p.show("document.getElementById('d') instanceof HTMLElement && HTMLDivElement.prototype instanceof HTMLElement"),
        "true"
    );
}

#[test]
fn reflection_kinds() {
    let mut p = Page::new(DOC);
    // string, boolean y enumerados.
    assert_eq!(
        p.show(
            "var d = document.createElement('div'); d.title = null; d.hidden = true; \
                d.dir = 'RTL'; [d.getAttribute('title'), d.getAttribute('hidden'), d.dir].join('|')"
        ),
        "null||rtl"
    );
    assert_eq!(p.show("d.dir = 'x'; d.dir"), "");
    // long con default, limited long (negativo → IndexSizeError), unsigned long.
    assert_eq!(
        p.show("var ol = document.createElement('ol'); [ol.start, (ol.setAttribute('start', ' 7x'), ol.start)].join()"),
        "1,7"
    );
    assert_eq!(
        p.show(
            "var i = document.createElement('input'); try { i.maxLength = -1 } catch (e) { e.name }"
        ),
        "IndexSizeError"
    );
    assert_eq!(p.show("i.maxLength"), "-1");
    assert_eq!(
        p.show("var c = document.createElement('canvas'); c.width = -1; [c.width, c.getAttribute('width')].join()"),
        "300,300"
    );
    // clamped, URL y CORS nullable.
    assert_eq!(
        p.show(
            "var td = document.createElement('td'); td.colSpan = 0; [td.colSpan, td.rowSpan].join()"
        ),
        "1,1"
    );
    assert_eq!(
        p.show("var img = document.createElement('img'); img.src = 'p.png'; img.src"),
        "https://example.com/dir/p.png"
    );
    assert_eq!(
        p.show("[img.crossOrigin, (img.crossOrigin = ''), img.crossOrigin, (img.crossOrigin = 'X'), img.crossOrigin].join('|')"),
        "||anonymous|X|anonymous"
    );
    // double limitado: los no positivos se ignoran.
    assert_eq!(
        p.show("var pr = document.createElement('progress'); pr.max = 0; pr.max = 5; pr.max"),
        "5"
    );
    // §2.3.4.3: el exponente puede seguir al punto.
    assert_eq!(p.show("pr.setAttribute('max', '1.e2'); pr.max"), "100");
    assert_eq!(
        p.show("var b = document.createElement('b'); b.accessKey = 'q r'; b.accessKeyLabel"),
        "Alt+Q"
    );
    assert_eq!(
        p.show("[document.bgColor = null, document.bgColor, document.body.getAttribute('bgcolor')].join('|')"),
        "||"
    );
    // Brand check.
    assert_eq!(
        p.show("try { Object.getOwnPropertyDescriptor(HTMLAnchorElement.prototype, 'target').get.call(d) } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn hyperlinks() {
    let mut p = Page::new(DOC);
    assert_eq!(
        p.show("var a = document.getElementById('a'); [a.href, a.protocol, a.host, a.pathname, a.search, a.hash, a.origin].join(' ')"),
        "https://example.com/x/y?q=1#h https: example.com /x/y ?q=1 #h https://example.com"
    );
    assert_eq!(
        p.show("a.pathname = '/z'; a.hash = 'k'; a.getAttribute('href')"),
        "https://example.com/z?q=1#k"
    );
    assert_eq!(p.show("String(a) === a.href && a.text"), "enlace");
}

#[test]
fn form_controls() {
    let mut p = Page::new(DOC);
    assert_eq!(
        p.show("var i = document.getElementById('i'); i.value = 'a\\nb'; [i.value, i.defaultValue, i.getAttribute('value')].join('|')"),
        "ab|v|v"
    );
    assert_eq!(
        p.show("i.type = 'number'; i.value = '12e3'; var ok = i.value; i.value = 'x'; [ok, i.value, i.valueAsNumber].join('|')"),
        "12e3||NaN"
    );
    assert_eq!(
        p.show("var c = document.createElement('input'); c.type = 'checkbox'; [c.value, c.checked, (c.click(), c.checked)].join()"),
        "on,false,true"
    );
    assert_eq!(
        p.show("var s = document.getElementById('s'); [s.type, s.length, s.selectedIndex, s.value].join()"),
        "select-one,2,1,2"
    );
    assert_eq!(
        p.show("s.value = 'uno'; [s.selectedIndex, s.item(0).text, s.item(0).index].join()"),
        "0,uno,0"
    );
    assert_eq!(
        p.show("var f = document.getElementById('f'); [f.length, i.form === f, f.action, f.method].join()"),
        "2,true,https://example.com/dir/page.html,get"
    );
    assert_eq!(
        p.show("var t = document.createElement('textarea'); t.value = 'x\\r\\ny'; [t.value.length, t.textLength, t.type].join()"),
        "3,3,textarea"
    );
}

#[test]
fn tables() {
    let mut p = Page::new(DOC);
    assert_eq!(
        p.show("var t = document.getElementById('t'); var r = t.insertRow(); r.insertCell().textContent = 'z'; \
                [t.querySelector ? 1 : 1, r.rowIndex, r.sectionRowIndex, r.firstChild.cellIndex].join()"),
        "1,1,1,0"
    );
    assert_eq!(
        p.show("var th = t.createTHead(); [t.tHead === th, t.createTHead() === th, t.firstChild === th].join()"),
        "true,true,true"
    );
    assert_eq!(
        p.show("t.deleteRow(-1); try { t.deleteRow(5) } catch (e) { e.name }"),
        "IndexSizeError"
    );
    assert_eq!(
        p.show("var cap = t.createCaption(); t.firstChild === cap"),
        "true"
    );
}

#[test]
fn html_element_api() {
    let mut p = Page::new(
        "<!DOCTYPE html><body><div id=d>uno <b>dos</b><p>tres</p>cuatro<br>cinco</div></body>",
    );
    assert_eq!(
        p.show("JSON.stringify(document.getElementById('d').innerText)"),
        "\"uno dos\\n\\ntres\\n\\ncuatro\\ncinco\""
    );
    assert_eq!(
        p.show("var d = document.getElementById('d'); d.innerText = 'a\\nb'; [d.childNodes ? 1 : 1, d.firstChild.nodeName, d.lastChild.data].join()"),
        "1,#text,b"
    );
    assert_eq!(
        p.show("var b = document.createElement('button'); var a = document.createElement('a'); [b.tabIndex, a.tabIndex, (a.href = 'x', a.tabIndex)].join()"),
        "0,-1,0"
    );
    assert_eq!(
        p.show("d.contentEditable = 'TRUE'; [d.contentEditable, d.isContentEditable].join()"),
        "true,true"
    );
    assert_eq!(
        p.show("document.createElement('template').content.nodeType"),
        "11"
    );
}

#[test]
fn live_collections_and_token_lists() {
    let mut p = Page::new(DOC);
    assert_eq!(
        p.show(
            "var t = document.getElementById('t'); var rows = t.rows; t.insertRow(); \
                [rows.length, rows === t.rows, t.tBodies.length, rows[0].cells.length].join()"
        ),
        "2,true,1,2"
    );
    assert_eq!(
        p.show(
            "var f = document.getElementById('f'); [f.elements.length, f.elements[0].id, \
                f.elements.n.id, document.getElementById('s').options.length, \
                document.getElementById('s').selectedOptions[0].value].join()"
        ),
        "2,i,i,2,2"
    );
    assert_eq!(
        p.show("var a = document.getElementById('a'); a.relList.add('noopener'); \
                a.relList = 'x y'; [a.getAttribute('rel'), a.relList.length, a.relList === a.relList].join()"),
        "x y,2,true"
    );
}
