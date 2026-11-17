//! E1: interfaces que faltaban y que los sitios usan al montar la página: las de SVG (SVG 2 §4,
//! §5; GitLab, YouTube), las de geometría (Geometry Interfaces 1; Apple) y
//! `HTMLSelectElement.add`/`remove(index)` (HTML §4.10.7; documentación de Python).

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext};

struct Page {
    rt: Runtime,
    realm: bf_js::vm::RealmId,
}

impl Page {
    fn new(html: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.test/", None).expect("url");
        bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, realm }
    }

    fn show(&mut self, src: &str) -> String {
        match self.rt.eval_script(self.realm, src, "test.js") {
            Ok(v) => self.rt.to_display(&v),
            Err(e) => panic!("error: {e}\n{}", e.stack),
        }
    }
}

/// SVG 2 §4.4: los elementos del namespace SVG son instancias de su interfaz; los desconocidos,
/// de `SVGElement`. `window.SVGElement.prototype` existe (lo que leía YouTube).
#[test]
fn svg_elements_have_their_interfaces() {
    let mut p = Page::new(
        "<div id=d></div><svg id=s class='a b'><g><circle id=c r=5 /><foo id=f /></g>\
         <text id=t>x</text></svg>",
    );
    assert_eq!(
        p.show(
            "const s = document.getElementById('s'), c = document.getElementById('c'); \
             const f = document.getElementById('f'), t = document.getElementById('t'); \
             const r = document.createElementNS('http://www.w3.org/2000/svg', 'rect'); \
             [typeof window.SVGElement.prototype, s instanceof SVGSVGElement, \
              s instanceof SVGGraphicsElement, s instanceof SVGElement, s instanceof Element, \
              c instanceof SVGCircleElement, c instanceof SVGGeometryElement, \
              Object.getPrototypeOf(f) === SVGElement.prototype, \
              t instanceof SVGTextPositioningElement, r instanceof SVGRectElement, \
              document.getElementById('d') instanceof SVGElement, \
              s.className.baseVal, c.ownerSVGElement === s, String(s.ownerSVGElement), \
              typeof s.dataset, typeof c.getBBox().width, s.createSVGPoint() instanceof DOMPoint, \
              c.getCTM().isIdentity, String(SVGPathElement.prototype.constructor.name)].join()"
        ),
        "object,true,true,true,true,true,true,true,true,true,false,a b,true,null,object,number,\
         true,true,SVGPathElement"
    );
    assert_eq!(
        p.show(
            "const k = document.getElementById('c'); k.className.baseVal = 'z'; \
             [k.getAttribute('class'), k.className.animVal].join()"
        ),
        "z,z"
    );
}

/// Geometry Interfaces 1 §2, §4, §6: constructores, `fromMatrix`, operaciones y la cadena.
#[test]
fn geometry_interfaces() {
    let mut p = Page::new("");
    assert_eq!(
        p.show(
            "const m = new DOMMatrix(); const t = m.translate(10, 20).scale(2); \
             const p = t.transformPoint({x: 1, y: 1}); \
             const r = new DOMMatrix('translate(5px, 6px) rotate(90deg)'); \
             const q = DOMQuad.fromRect({x: 1, y: 2, width: 3, height: 4}).getBounds(); \
             const i = new DOMMatrix([2, 0, 0, 2, 4, 6]).inverse(); \
             [typeof DOMMatrix, m.isIdentity, m.is2D, String(t), p.x, p.y, \
              Math.round(r.b), r.e, r.f, q.x, q.y, q.width, q.height, String(i), \
              new DOMMatrix([1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,7,1]).is2D, \
              DOMMatrix.fromMatrix({a: 3}).m11, new DOMPoint(1, 2).toJSON().w, \
              new DOMPointReadOnly(1).x, DOMMatrix.length, WebKitCSSMatrix === DOMMatrix, \
              String(new DOMMatrix('none')), m instanceof DOMMatrixReadOnly].join('|')"
        ),
        "function|true|true|matrix(2, 0, 0, 2, 10, 20)|12|22|1|5|6|1|2|3|4|\
         matrix(0.5, 0, 0, 0.5, -2, -3)|false|3|1|1|0|true|matrix(1, 0, 0, 1, 0, 0)|true"
    );
    assert_eq!(
        p.show(
            "function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
             [err(() => new DOMMatrix('banana(1)')), err(() => new DOMMatrix([1, 2, 3])), \
              err(() => DOMMatrix()), err(() => DOMMatrix.fromMatrix({a: 1, m11: 2})), \
              err(() => String(new DOMMatrix([NaN, 0, 0, 1, 0, 0])))].join()"
        ),
        "SyntaxError,TypeError,TypeError,TypeError,InvalidStateError"
    );
}

/// HTML §4.10.7: `add(element, before)` y `remove(index)`; `remove()` sin argumentos sigue
/// quitando el `select` (ChildNode).
#[test]
fn select_add_and_remove() {
    let mut p = Page::new("<div id=d><select id=s><option>a<option>b</select></div>");
    assert_eq!(
        p.show(
            "const s = document.getElementById('s'); \
             const opt = (t) => { const o = document.createElement('option'); o.text = t; return o; }; \
             s.add(opt('c')); s.add(opt('0'), 0); s.add(opt('x'), s.options[2]); \
             const after = Array.from(s.options, (o) => o.text).join(''); \
             s.remove(1); s.remove(99); \
             const removed = Array.from(s.options, (o) => o.text).join(''); \
             function err(f) { try { f(); return 'ok'; } catch (e) { return e.name; } } \
             const e1 = err(() => s.add(document.getElementById('d'))); \
             const e2 = err(() => s.add(opt('q'), document.createElement('option'))); \
             s.remove(); \
             [after, removed, e1, e2, document.getElementById('s')].join()"
        ),
        "0axbc,0xbc,TypeError,NotFoundError,"
    );
}
