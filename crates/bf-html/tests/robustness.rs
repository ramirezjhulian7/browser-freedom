//! E3: casos del parser que antes colgaban el motor o corrían scripts de más o de menos:
//! ciclos del adoption agency a través de un shadow root o del contenido de un template,
//! scripts SVG (HTML §13.2.6.5), el punto de control de microtasks antes de cada script
//! (§13.2.6.4.8, paso 1) y CR LF partidos entre dos `document.write`.

mod common;

use bf_dom::events::Event;
use bf_dom::shadow::ShadowRootInit;
use bf_dom::{Document, DomString, NodeData, NodeId};
use bf_html::{ParseOptions, Parser, ScriptHost, SvgScriptSource, svg_script_source};
use common::TestHost;

fn opts() -> ParseOptions {
    ParseOptions {
        scripting: true,
        ..ParseOptions::default()
    }
}

/// Un host que corre `on_execute` en cada script y anota los puntos de control y los
/// scripts SVG que el parser le pide procesar.
struct Host<F: FnMut(&mut Parser)> {
    on_execute: F,
    checkpoints: usize,
    svg: Vec<Option<SvgScriptSource>>,
}

impl<F: FnMut(&mut Parser)> Host<F> {
    fn new(on_execute: F) -> Self {
        Host {
            on_execute,
            checkpoints: 0,
            svg: Vec::new(),
        }
    }
}

impl<F: FnMut(&mut Parser)> bf_dom::events::ScriptHost for Host<F> {
    fn call_listener(&mut self, _: &mut Document, _: u64, _: &mut Event) -> bool {
        false
    }
}

impl<F: FnMut(&mut Parser)> ScriptHost for Host<F> {
    fn execute(&mut self, parser: &mut Parser, _: NodeId) {
        (self.on_execute)(parser);
    }

    fn microtask_checkpoint(&mut self, _: &mut Parser) {
        self.checkpoints += 1;
    }

    fn process_svg_script(&mut self, parser: &mut Parser, script: NodeId) {
        self.svg.push(svg_script_source(parser.document(), script));
    }
}

fn parse(host: &mut dyn ScriptHost, html: &str) -> Parser {
    let mut parser = Parser::with_input(html, &opts());
    parser.finish(host);
    parser
}

fn id(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(&DomString::from(id)).expect(id)
}

/// `html/syntax/parsing/adoption-agency-reparenting-shadow-cycle.html`: el script mete el
/// common ancestor (`#ca`) en un shadow root cuyo host cuelga del furthest block (`#fb`).
/// Insertar `#fb` en `#ca` haría un ciclo que cruza el shadow root: el parser lo tira y
/// termina (antes no terminaba).
#[test]
fn adoption_agency_does_not_build_a_cycle_through_a_shadow_root() {
    let ids = std::cell::Cell::new(None);
    let mut host = Host::new(|p: &mut Parser| {
        let doc = p.document_mut();
        let (fb, ca) = (id(doc, "fb"), id(doc, "ca"));
        ids.set(Some((fb, ca)));
        let span = doc.create_element(&DomString::from("span")).unwrap();
        let root = doc.attach_shadow(span, ShadowRootInit::default()).unwrap();
        let body = doc.body().unwrap();
        doc.append_child(body, fb).unwrap();
        doc.append_child(fb, span).unwrap();
        doc.append_child(root, ca).unwrap();
    });
    let p = parse(
        &mut host,
        "<div id=ca><b><p id=fb><script>x</script></b></div>",
    );
    let doc = p.document();
    let (fb, ca) = ids.get().expect("el script corrió");
    assert!(!doc.is_inclusive_ancestor(ca, fb), "#fb no entra en #ca");
    assert_eq!(doc.parent(fb), None, "#fb queda sin padre");
}

/// El mismo ciclo a través del contenido de un `template` creado por script (que el parser no
/// tiene en su mapa de templates).
#[test]
fn adoption_agency_does_not_build_a_cycle_through_template_contents() {
    let ids = std::cell::Cell::new(None);
    let mut host = Host::new(|p: &mut Parser| {
        let doc = p.document_mut();
        let (fb, ca) = (id(doc, "fb"), id(doc, "ca"));
        ids.set(Some((fb, ca)));
        let tmpl = doc.create_element(&DomString::from("template")).unwrap();
        let contents = doc.element(tmpl).and_then(|e| e.template_contents).unwrap();
        let body = doc.body().unwrap();
        doc.append_child(body, fb).unwrap();
        doc.append_child(fb, tmpl).unwrap();
        doc.append_child(contents, ca).unwrap();
    });
    let p = parse(
        &mut host,
        "<div id=ca><b><p id=fb><script>x</script></b></div>",
    );
    let doc = p.document();
    let (fb, ca) = ids.get().expect("el script corrió");
    assert!(!doc.is_inclusive_ancestor(ca, fb));
    assert_eq!(doc.parent(fb), None);
}

/// `html/syntax/parsing/unclosed-svg-script.html`: el parser pide procesar el `script` SVG
/// que se cierra con su end tag o con self-closing, no el que corta otro end tag; uno con
/// `type` que no es de JavaScript no corre.
#[test]
fn svg_scripts_are_processed_when_their_end_tag_is_seen() {
    let mut host = Host::new(|_: &mut Parser| {});
    parse(
        &mut host,
        "<svg><script>a()</script></svg>\
         <svg><script href='x.js'/></svg>\
         <svg><script>b()</svg>\
         <svg><script type=text/plain>c</script></svg>",
    );
    assert_eq!(
        host.svg,
        [
            Some(SvgScriptSource::Inline("a()".into())),
            Some(SvgScriptSource::External("x.js".into())),
            None,
        ]
    );
}

/// Un punto de control de microtasks antes de preparar cada script del parser.
#[test]
fn microtask_checkpoint_before_each_parser_script() {
    let mut host = Host::new(|_: &mut Parser| {});
    parse(&mut host, "<script>a</script><p><script>b</script>");
    assert_eq!(host.checkpoints, 2);
}

/// `html5lib_write_single.html`: `\r` y `\n` en dos `write()` seguidos son un solo salto.
#[test]
fn cr_and_lf_in_separate_writes_are_one_newline() {
    let mut host = TestHost::new();
    let p = parse(
        &mut host,
        "<pre><script>document.write('a\\r');document.write('\\nb')</script></pre>",
    );
    let doc = p.document();
    let text: String = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .filter(|&n| {
            !doc.parent(n)
                .and_then(|p| doc.element(p))
                .is_some_and(|e| e.is_html("script"))
        })
        .filter_map(|n| match &doc.get(n).data {
            NodeData::Text(t) => Some(t.as_lossy_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "a\nb");
}
