//! Scripts en el parser (HTML §13.2.6.4.8, §4.12.1, §8.4, §13.2.7) con un host de prueba que
//! registra el orden de preparación y ejecución y corre `document.write` literales.

mod common;

use bf_dom::events::{EventTarget, Listener, ListenerOptions};
use bf_dom::script::{CorsSetting, DocumentReadiness, FetchPriority, ScriptType};
use bf_dom::{Document, DomException, DomString, NodeData, NodeId};
use bf_html::{
    FragmentContext, ParseOptions, Parser, ParserStatus, ScriptDisposition, ScriptHost,
    parse_fragment,
};
use bf_infra::Namespace;
use common::TestHost;

fn opts() -> ParseOptions {
    ParseOptions {
        scripting: true,
        iframe_srcdoc: false,
    }
}

const BASE: &str = "https://example.test/dir/page.html";

/// Parser sobre `html` completo, para un documento en [`BASE`].
fn parser_for(html: &str, opts: &ParseOptions) -> Parser {
    let mut parser = Parser::with_input(html, opts);
    parser.document_mut().url = bf_infra::Url::parse(BASE, None).unwrap();
    parser
}

/// Parser de red vacío, para un documento en [`BASE`].
fn streaming_parser() -> Parser {
    let mut parser = Parser::new(&opts());
    parser.document_mut().url = bf_infra::Url::parse(BASE, None).unwrap();
    parser
}

/// Parsea `html` completo con `host`.
fn parse_with(host: &mut dyn ScriptHost, html: &str) -> Parser {
    let mut parser = parser_for(html, &opts());
    parser.finish(host);
    parser
}

/// Texto del body sin el de los `script`.
fn text(doc: &Document) -> String {
    let mut out = String::new();
    let Some(body) = doc.body() else {
        return out;
    };
    for n in doc.inclusive_descendants(body) {
        if let NodeData::Text(t) = &doc.get(n).data {
            let in_script = doc
                .parent(n)
                .and_then(|p| doc.element(p))
                .is_some_and(|e| e.is_html("script"));
            if !in_script {
                out.push_str(t.as_lossy_str());
            }
        }
    }
    out
}

fn tree(parser: &Parser) -> String {
    let doc = parser.document();
    bf_dom::test_serialize(doc, doc.root())
}

/// Un listener en `target` que el host registra con `name`.
fn listen(parser: &mut Parser, host: &mut TestHost, target: EventTarget, type_: &str, name: &str) {
    let handle = host.listeners.len() as u64 + 1;
    host.listeners.insert(handle, name.to_owned());
    parser.document_mut().add_event_listener(
        target,
        type_,
        Listener::Script(handle),
        ListenerOptions {
            capture: true,
            ..ListenerOptions::default()
        },
    );
}

/// Escucha `load` y `error` de los elementos (en captura, desde el documento).
fn listen_script_events(parser: &mut Parser, host: &mut TestHost) {
    let root = EventTarget::Node(parser.document().root());
    listen(parser, host, root, "load", "on");
    listen(parser, host, root, "error", "on");
}

fn scripts(doc: &Document) -> Vec<NodeId> {
    doc.inclusive_descendants(doc.root())
        .into_iter()
        .filter(|&n| doc.element(n).is_some_and(|e| e.is_html("script")))
        .collect()
}

fn first_script(doc: &Document) -> NodeId {
    scripts(doc)[0]
}

/// Host con un closure para `execute` (scripts que llaman al DOM).
struct FnHost<F: FnMut(&mut Parser, NodeId)> {
    f: F,
}

impl<F: FnMut(&mut Parser, NodeId)> bf_dom::events::ScriptHost for FnHost<F> {
    fn call_listener(&mut self, _: &mut Document, _: u64, _: &mut bf_dom::events::Event) -> bool {
        false
    }
}

impl<F: FnMut(&mut Parser, NodeId)> ScriptHost for FnHost<F> {
    fn execute(&mut self, parser: &mut Parser, script: NodeId) {
        (self.f)(parser, script);
    }
}

// --- document.write ---

#[test]
fn write_inserts_at_insertion_point() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "1<script>document.write(\"2\")</script>3");
    assert_eq!(text(p.document()), "123");
    assert_eq!(
        host.log,
        [
            "prepare:document.write(\"2\")",
            "execute:document.write(\"2\")"
        ]
    );
}

#[test]
fn nested_written_scripts_run_in_order() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "1<script>document.write(\"<script>document.write('2')</scr\"+ \"ipt><script>document.write('3')</scr\" + \"ipt>\")</script>4",
    );
    assert_eq!(text(p.document()), "1234");
    assert_eq!(scripts(p.document()).len(), 3);
}

#[test]
fn three_levels_of_nested_writes() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        r#"a<script id=s1>document.write("<script id=s2>document.write('<script id=s3>document.write(\\'z\\')</scr' + 'ipt>y')</scr" + "ipt>x")</script>b"#,
    );
    assert_eq!(text(p.document()), "azyxb", "{:?}", host.log);
    let order: Vec<_> = host
        .log
        .iter()
        .filter(|l| l.starts_with("execute"))
        .collect();
    assert_eq!(order, ["execute:s1", "execute:s2", "execute:s3"]);
}

#[test]
fn multiple_writes_keep_order() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "<script>document.write('a'); document.write('b', 'c')</script>d",
    );
    assert_eq!(text(p.document()), "abcd");
}

#[test]
fn writeln_appends_newline() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "<pre><script>document.writeln('x')</script>y</pre>",
    );
    assert_eq!(text(p.document()), "x\ny");
}

#[test]
fn partial_tag_across_writes() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "<script>document.write('<b'); document.write(' id=q>x</b>')</script>",
    );
    let doc = p.document();
    let b = doc.get_element_by_id(&"q".into()).expect("b#q");
    assert!(doc.element(b).unwrap().is_html("b"));
    assert_eq!(text(doc), "x");
}

#[test]
fn unclosed_written_markup_continues_with_rest() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script>document.write('<b>')</script>x");
    assert!(
        tree(&p).contains("|     <b>\n|       \"x\""),
        "{}",
        tree(&p)
    );
}

#[test]
fn written_markup_is_parsed_synchronously() {
    let mut seen = Vec::new();
    let mut host = FnHost {
        f: |p: &mut Parser, _| {
            p.write(&mut NoopHost, &"<p id=w>".into()).unwrap();
            seen.push(p.document().get_element_by_id(&"w".into()).is_some());
        },
    };
    parse_with(&mut host, "<script>x</script>");
    assert_eq!(seen, [true]);
}

/// Host sin efectos para llamadas anidadas desde un closure.
struct NoopHost;

impl bf_dom::events::ScriptHost for NoopHost {
    fn call_listener(&mut self, _: &mut Document, _: u64, _: &mut bf_dom::events::Event) -> bool {
        false
    }
}

impl ScriptHost for NoopHost {}

#[test]
fn write_lone_surrogate() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script>document.write('a\\uD800b')</script>");
    let doc = p.document();
    let body = doc.body().unwrap();
    let t = doc.get(doc.last_child(body).unwrap()).data.clone();
    let NodeData::Text(t) = t else {
        panic!("se esperaba texto");
    };
    assert_eq!(t.code_points().collect::<Vec<_>>(), [0x61, 0xD800, 0x62]);
}

#[test]
fn write_normalizes_newlines() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script>document.write('a\\r\\nb\\rc')</script>");
    assert_eq!(text(p.document()), "a\nb\nc");
}

#[test]
fn nesting_level_and_insertion_point() {
    let mut seen = Vec::new();
    let mut host = FnHost {
        f: |p: &mut Parser, _| seen.push((p.script_nesting_level(), p.insertion_point().is_some())),
    };
    let p = parse_with(&mut host, "<script>a</script><script>b</script>");
    assert_eq!(seen, [(1, true), (1, true)]);
    assert_eq!(p.script_nesting_level(), 0);
    assert_eq!(p.insertion_point(), None);
}

#[test]
fn nested_script_has_nesting_level_two() {
    let mut levels = Vec::new();
    let mut host = FnHost {
        f: |p: &mut Parser, s: NodeId| {
            levels.push(p.script_nesting_level());
            if p.document().element(s).unwrap().attr("id") == Some("outer") {
                let mut inner = FnHost {
                    f: |p: &mut Parser, _| assert_eq!(p.script_nesting_level(), 2),
                };
                p.write(&mut inner, &"<script>inner</script>".into())
                    .unwrap();
            }
        },
    };
    parse_with(&mut host, "<script id=outer>x</script>");
    assert_eq!(levels, [1]);
}

#[test]
fn write_after_load_replaces_document() {
    let mut host = TestHost::new();
    let mut p = parse_with(&mut host, "<p>old</p>");
    assert_eq!(p.status(), ParserStatus::Complete);
    // Sin insertion point: document.write abre el documento de nuevo.
    p.write(&mut host, &"<p>new".into()).unwrap();
    assert_eq!(p.document().ready_state(), DocumentReadiness::Loading);
    assert_eq!(text(p.document()), "new");
    p.close(&mut host).unwrap();
    p.run_tasks(&mut host);
    assert_eq!(p.document().ready_state(), DocumentReadiness::Complete);
    assert_eq!(p.status(), ParserStatus::Complete);
}

#[test]
fn open_during_parser_script_is_noop() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "a<script>document.open(); document.write('b')</script>c",
    );
    assert_eq!(text(p.document()), "abc");
}

#[test]
fn open_write_close_cycle() {
    let mut host = TestHost::new();
    let mut p = parse_with(&mut host, "<title>x</title>");
    p.open(&mut host).unwrap();
    assert_eq!(p.document().document_element(), None);
    p.write(&mut host, &"<!doctype html><p>uno".into()).unwrap();
    p.write(&mut host, &"<p>dos".into()).unwrap();
    p.close(&mut host).unwrap();
    p.run_tasks(&mut host);
    assert_eq!(text(p.document()), "unodos");
    assert!(p.document().doctype().is_some());
    // close() sin parser creado por script no hace nada.
    p.close(&mut host).unwrap();
}

#[test]
fn open_erases_listeners() {
    let mut host = TestHost::new();
    let mut p = parse_with(&mut host, "x");
    let root = EventTarget::Node(p.document().root());
    listen(&mut p, &mut host, root, "DOMContentLoaded", "dcl");
    p.open(&mut host).unwrap();
    p.close(&mut host).unwrap();
    p.run_tasks(&mut host);
    assert!(
        host.log.iter().all(|l| !l.starts_with("dcl")),
        "{:?}",
        host.log
    );
}

#[test]
fn write_in_xml_document_throws() {
    let mut host = TestHost::new();
    let mut p = streaming_parser();
    p.document_mut().is_html = false;
    assert_eq!(
        p.write(&mut host, &"x".into()),
        Err(DomException::InvalidStateError)
    );
    assert_eq!(p.open(&mut host), Err(DomException::InvalidStateError));
    assert_eq!(p.close(&mut host), Err(DomException::InvalidStateError));
}

#[test]
fn throw_on_dynamic_markup_insertion() {
    let mut host = TestHost::new();
    let mut p = streaming_parser();
    p.document_mut()
        .scripts_mut()
        .throw_on_dynamic_markup_insertion = 1;
    assert_eq!(
        p.write(&mut host, &"x".into()),
        Err(DomException::InvalidStateError)
    );
}

#[test]
fn blocking_external_script_can_write() {
    let mut host =
        TestHost::new().with_external("a.js", "document.write('W')", ScriptDisposition::Ready);
    let p = parse_with(&mut host, "<script src=a.js></script>x");
    assert_eq!(text(p.document()), "Wx");
}

#[test]
fn async_external_write_is_ignored() {
    let mut host =
        TestHost::new().with_external("a.js", "document.write('W')", ScriptDisposition::Ready);
    let p = parse_with(&mut host, "<script async src=a.js></script>x");
    assert_eq!(text(p.document()), "x");
    assert!(host.log.contains(&"execute:a.js".to_owned()));
}

#[test]
fn feed_in_chunks_matches_whole_input() {
    let html = "1<script>document.write('<i>2</i>')</script>3<script>document.write('4')</script>5";
    let mut whole = TestHost::new();
    let a = parse_with(&mut whole, html);
    for size in [1, 2, 3, 7] {
        let mut host = TestHost::new();
        let mut p = streaming_parser();
        let chars: Vec<char> = html.chars().collect();
        for chunk in chars.chunks(size) {
            p.feed(&chunk.iter().collect::<String>(), &mut host);
        }
        p.finish(&mut host);
        assert_eq!(tree(&p), tree(&a), "trozos de {size}");
        assert_eq!(host.log, whole.log, "trozos de {size}");
    }
}

#[test]
fn script_waits_for_its_end_tag() {
    let mut host = TestHost::new();
    let mut p = streaming_parser();
    p.feed("<script>log('x')</scr", &mut host);
    assert!(host.log.is_empty());
    p.feed("ipt>", &mut host);
    assert_eq!(host.log, ["prepare:log('x')", "execute:log('x')", "x"]);
    p.finish(&mut host);
}

// --- prepare the script element ---

#[test]
fn classification_of_external_module() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "<script type=' Module ' src='m.js' async defer nomodule charset=latin1 crossorigin=USE-CREDENTIALS integrity='sha384-x' nonce=n1 referrerpolicy=ORIGIN fetchpriority=High></script>",
    );
    let doc = p.document();
    let info = doc.script_info(first_script(doc)).expect("preparado");
    assert_eq!(info.type_, ScriptType::Module);
    assert_eq!(info.type_string, "Module");
    assert_eq!(info.src.as_ref().map(|s| s.as_lossy_str()), Some("m.js"));
    assert!(info.url.is_some());
    assert!(info.is_async && info.defer && info.nomodule && info.parser_inserted);
    assert_eq!(info.encoding, "windows-1252");
    assert_eq!(info.crossorigin, CorsSetting::UseCredentials);
    assert_eq!(info.integrity, "sha384-x");
    assert_eq!(info.nonce, "n1");
    assert_eq!(info.referrer_policy, "origin");
    assert_eq!(info.fetch_priority, FetchPriority::High);
}

#[test]
fn classification_defaults() {
    let mut host = TestHost::new();
    let p = parse_with(
        &mut host,
        "<script crossorigin=x referrerpolicy=bogus>1</script>",
    );
    let doc = p.document();
    let info = doc.script_info(first_script(doc)).unwrap();
    assert_eq!(info.type_, ScriptType::Classic);
    assert_eq!(info.type_string, "text/javascript");
    assert_eq!(info.source_text, "1");
    assert_eq!(info.encoding, "UTF-8");
    assert_eq!(info.crossorigin, CorsSetting::Anonymous);
    assert_eq!(info.referrer_policy, "");
    assert_eq!(info.fetch_priority, FetchPriority::Auto);
    assert!(!info.is_async && !info.defer && info.src.is_none());
}

#[test]
fn script_types() {
    let cases = [
        ("", true),
        (" type=''", true),
        (" type=TEXT/JavaScript", true),
        (" type=application/x-ecmascript", true),
        (" type=module", true),
        (" type=importmap", true),
        (" type=text/plain", false),
        (" type='text/javascript; charset=utf-8'", false),
        (" language=''", true),
        (" language=javascript", true),
        (" language=vbscript", false),
        (" type=text/javascript language=vbscript", true),
    ];
    for (attrs, runs) in cases {
        let mut host = TestHost::new();
        parse_with(&mut host, &format!("<script{attrs}>{{}}</script>"));
        assert_eq!(!host.log.is_empty(), runs, "<script{attrs}>");
    }
}

#[test]
fn nomodule_classic_is_not_run() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script nomodule>log('x')</script>");
    assert!(host.log.is_empty());
    assert!(
        p.document()
            .script_state(first_script(p.document()))
            .already_started
    );
}

#[test]
fn empty_inline_script_is_not_prepared() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script></script>");
    assert!(host.log.is_empty());
    let state = p.document().script_state(first_script(p.document()));
    assert!(!state.already_started);
    assert!(!state.parser_inserted);
}

#[test]
fn scripting_disabled() {
    let mut host = TestHost::new();
    let mut p = parser_for(
        "<script>log('x')</script>",
        &ParseOptions {
            scripting: false,
            iframe_srcdoc: false,
        },
    );
    p.finish(&mut host);
    assert!(host.log.is_empty());
    assert!(
        p.document()
            .script_state(first_script(p.document()))
            .already_started
    );
}

#[test]
fn for_and_event_attributes() {
    let mut host = TestHost::new();
    parse_with(
        &mut host,
        "<script id=a for=' WINDOW ' event='onload()'>;</script><script id=b for=document event=onload>;</script><script id=c for=window event=onclick>;</script>",
    );
    assert_eq!(host.log, ["prepare:a", "execute:a"]);
}

#[test]
fn fragment_scripts_are_already_started() {
    let (doc, _) = parse_fragment(
        "<script>x</script>",
        FragmentContext {
            ns: Namespace::Html,
            name: "div",
        },
        &opts(),
    );
    let script = first_script(&doc);
    let state = doc.script_state(script);
    assert!(state.already_started && state.parser_inserted);
}

#[test]
fn script_cut_by_eof_is_already_started() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script>log('x')");
    assert!(host.log.is_empty());
    assert!(
        p.document()
            .script_state(first_script(p.document()))
            .already_started
    );
}

#[test]
fn bad_src_fires_error() {
    let mut host = TestHost::new();
    let mut p = parser_for(
        "<script id=e1 src=''></script><script id=e2 type=importmap src=x.json></script><script id=e3 src='http://[bad'></script>",
        &opts(),
    );
    listen_script_events(&mut p, &mut host);
    p.finish(&mut host);
    assert_eq!(
        host.log,
        [
            "on:error@e1(interactive)",
            "on:error@e2(interactive)",
            "on:error@e3(interactive)"
        ]
    );
}

#[test]
fn failed_fetch_fires_error_and_success_fires_load() {
    let mut host = TestHost::new()
        .with_external("ok.js", "log('ok')", ScriptDisposition::Ready)
        .with_external("bad.js", "", ScriptDisposition::Failed);
    let mut p = parser_for(
        "<script src=ok.js></script><script src=bad.js></script><script>log('inline')</script>",
        &opts(),
    );
    listen_script_events(&mut p, &mut host);
    p.finish(&mut host);
    assert_eq!(
        host.log,
        [
            "prepare:ok.js",
            "execute:ok.js",
            "ok",
            "on:load@ok.js(loading)",
            "prepare:bad.js",
            "on:error@bad.js(loading)",
            "prepare:log('inline')",
            "execute:log('inline')",
            "inline",
        ]
    );
}

// --- Listas de scripts y bloqueo del parser ---

#[test]
fn parsing_blocking_script_pauses_parser() {
    let mut host = TestHost::new().with_external("b.js", "log('b')", ScriptDisposition::Pending);
    let mut p = parser_for("<script src=b.js></script><p id=after>x</p>", &opts());
    p.finish(&mut host);
    let script = first_script(p.document());
    assert_eq!(p.status(), ParserStatus::Blocked(script));
    assert!(p.document().get_element_by_id(&"after".into()).is_none());
    assert_eq!(p.document().ready_state(), DocumentReadiness::Loading);
    p.script_fetched(&mut host, script, true);
    assert!(p.document().get_element_by_id(&"after".into()).is_some());
    assert_eq!(p.status(), ParserStatus::Complete);
    assert_eq!(host.log, ["prepare:b.js", "execute:b.js", "b"]);
}

#[test]
fn feed_while_blocked_buffers_input() {
    let mut host = TestHost::new().with_external("b.js", "log('b')", ScriptDisposition::Pending);
    let mut p = streaming_parser();
    p.feed("<script src=b.js></script>1", &mut host);
    p.feed("<script>log('2')</script>", &mut host);
    assert_eq!(host.log, ["prepare:b.js"]);
    let script = first_script(p.document());
    p.script_fetched(&mut host, script, true);
    assert_eq!(
        host.log,
        [
            "prepare:b.js",
            "execute:b.js",
            "b",
            "prepare:log('2')",
            "execute:log('2')",
            "2"
        ]
    );
    p.finish(&mut host);
    assert_eq!(text(p.document()), "1");
}

#[test]
fn written_blocking_script_pauses_nested_parse() {
    let mut host = TestHost::new().with_external("w.js", "log('w')", ScriptDisposition::Pending);
    let mut p = parser_for(
        "<script>document.write('<script src=w.js></scr' + 'ipt>X'); log('after write')</script>Y",
        &opts(),
    );
    p.finish(&mut host);
    // El write insertó todo, pero el parser se pausó en el script bloqueante.
    assert_eq!(text(p.document()), "");
    assert!(host.log.contains(&"after write".to_owned()));
    let w = scripts(p.document())[1];
    assert_eq!(p.status(), ParserStatus::Blocked(w));
    p.script_fetched(&mut host, w, true);
    assert_eq!(text(p.document()), "XY");
    assert_eq!(host.log.last().map(String::as_str), Some("w"));
}

#[test]
fn defer_scripts_run_in_order_before_domcontentloaded() {
    let mut host = TestHost::new()
        .with_external("d1.js", "log('d1')", ScriptDisposition::Pending)
        .with_external("d2.js", "log('d2')", ScriptDisposition::Pending);
    let mut p = parser_for(
        "<script defer src=d1.js></script><script defer src=d2.js></script><script>log('inline')</script>",
        &opts(),
    );
    let root = EventTarget::Node(p.document().root());
    listen(&mut p, &mut host, root, "DOMContentLoaded", "dcl");
    p.finish(&mut host);
    assert_eq!(p.status(), ParserStatus::Finishing);
    assert_eq!(p.document().ready_state(), DocumentReadiness::Interactive);
    let s = scripts(p.document());
    // d2 llega primero, pero se ejecuta después de d1.
    p.script_fetched(&mut host, s[1], true);
    assert!(!host.log.contains(&"d2".to_owned()));
    p.script_fetched(&mut host, s[0], true);
    let tail: Vec<_> = host.log.iter().skip_while(|l| *l != "inline").collect();
    assert_eq!(
        tail,
        [
            "inline",
            "execute:d1.js",
            "d1",
            "execute:d2.js",
            "d2",
            "dcl:DOMContentLoaded@document(interactive)"
        ]
    );
    assert_eq!(p.status(), ParserStatus::Complete);
}

#[test]
fn async_scripts_run_when_ready_and_delay_load() {
    let mut host = TestHost::new()
        .with_external("a1.js", "log('a1')", ScriptDisposition::Pending)
        .with_external("a2.js", "log('a2')", ScriptDisposition::Pending);
    let mut p = parser_for(
        "<script async src=a1.js></script><script async src=a2.js></script>",
        &opts(),
    );
    listen(&mut p, &mut host, EventTarget::Window, "load", "win");
    p.finish(&mut host);
    assert_eq!(p.status(), ParserStatus::Finishing);
    let s = scripts(p.document());
    p.script_fetched(&mut host, s[1], true);
    assert_eq!(host.log.last().map(String::as_str), Some("a2"));
    assert!(!host.log.iter().any(|l| l.starts_with("win")));
    p.script_fetched(&mut host, s[0], true);
    assert_eq!(
        host.log[host.log.len() - 2..],
        ["a1", "win:load@document(complete)"]
    );
}

#[test]
fn inline_module_is_deferred() {
    let mut host = TestHost::new();
    parse_with(
        &mut host,
        "<script type=module>log('m')</script><script>log('c')</script>",
    );
    let logs: Vec<_> = host
        .log
        .iter()
        .filter(|l| l.len() == 1)
        .map(String::as_str)
        .collect();
    assert_eq!(logs, ["c", "m"]);
}

#[test]
fn async_module_runs_as_soon_as_possible() {
    let mut host = TestHost::new();
    let p = parse_with(&mut host, "<script type=module async>log('m')</script>");
    let doc = p.document();
    assert!(doc.scripts().as_soon_as_possible.is_empty());
    assert!(host.log.contains(&"m".to_owned()));
}

#[test]
fn dom_inserted_scripts() {
    let mut host = TestHost::new()
        .with_external("x.js", "log('x')", ScriptDisposition::Pending)
        .with_external("y.js", "log('y')", ScriptDisposition::Pending);
    let mut p = parse_with(&mut host, "<body>");
    let body = p.document().body().unwrap();
    // Inline: se ejecuta en el acto.
    let doc = p.document_mut();
    let inline = doc.create_element(&"script".into()).unwrap();
    let t = doc.create_text_node("log('i')".into());
    doc.append(inline, t);
    doc.append(body, inline);
    p.prepare_script(&mut host, inline);
    assert_eq!(host.log.last().map(String::as_str), Some("i"));
    // Externos: con force async van al set async; sin él, a la lista en orden.
    let doc = p.document_mut();
    let x = doc.create_element(&"script".into()).unwrap();
    doc.set_attribute(x, &"src".into(), "x.js".into()).unwrap();
    doc.append(body, x);
    let y = doc.create_element(&"script".into()).unwrap();
    doc.set_attribute(y, &"src".into(), "y.js".into()).unwrap();
    doc.script_state_mut(y).force_async = false;
    doc.append(body, y);
    p.prepare_script(&mut host, x);
    p.prepare_script(&mut host, y);
    assert_eq!(p.document().scripts().as_soon_as_possible, [x]);
    assert_eq!(p.document().scripts().in_order, [y]);
    p.script_fetched(&mut host, y, true);
    p.script_fetched(&mut host, x, true);
    let order: Vec<_> = host.log.iter().filter(|l| *l == "x" || *l == "y").collect();
    assert_eq!(order, ["y", "x"]);
}

#[test]
fn current_script() {
    let mut seen = Vec::new();
    let mut host = FnHost {
        f: |p: &mut Parser, s: NodeId| seen.push(p.document().scripts().current_script == Some(s)),
    };
    let p = parse_with(&mut host, "<script>1</script>");
    assert_eq!(seen, [true]);
    assert_eq!(p.document().scripts().current_script, None);
}

// --- readyState y eventos del documento ---

#[test]
fn ready_state_and_document_events() {
    let mut host = TestHost::new();
    let mut p = parser_for("<script>log('s')</script>", &opts());
    assert_eq!(p.document().ready_state(), DocumentReadiness::Loading);
    let root = EventTarget::Node(p.document().root());
    listen(&mut p, &mut host, root, "readystatechange", "doc");
    listen(&mut p, &mut host, root, "DOMContentLoaded", "doc");
    listen(&mut p, &mut host, EventTarget::Window, "load", "win");
    listen(
        &mut p,
        &mut host,
        EventTarget::Window,
        "DOMContentLoaded",
        "win",
    );
    p.finish(&mut host);
    assert_eq!(
        host.log[3..],
        [
            "doc:readystatechange@document(interactive)",
            "win:DOMContentLoaded@document(interactive)",
            "doc:DOMContentLoaded@document(interactive)",
            "doc:readystatechange@document(complete)",
            "win:load@document(complete)",
        ]
    );
}

#[test]
fn new_document_is_complete() {
    assert_eq!(Document::new().ready_state(), DocumentReadiness::Complete);
}

// --- Los escenarios de scripted_*.dat que llaman al DOM (el JS hecho a mano en Rust) ---

#[test]
fn scripted_adoption01_equivalent() {
    // document.getElementById("A").id = "B"
    let mut host = FnHost {
        f: |p: &mut Parser, _| {
            let doc = p.document_mut();
            let a = doc.get_element_by_id(&"A".into()).unwrap();
            doc.set_attribute(a, &"id".into(), "B".into()).unwrap();
        },
    };
    let p = parse_with(&mut host, "<p><b id=\"A\"><script>x</script></p>TEXT</b>");
    assert_eq!(
        tree(&p),
        "| <html>\n|   <head>\n|   <body>\n|     <p>\n|       <b>\n|         id=\"B\"\n|         <script>\n|           \"x\"\n|     <b>\n|       id=\"A\"\n|       \"TEXT\""
    );
}

#[test]
fn scripted_ark_equivalent() {
    // document.getElementsByTagName("font")[2].setAttribute("size", "5")
    let mut host = FnHost {
        f: |p: &mut Parser, _| {
            let doc = p.document_mut();
            let fonts: Vec<_> = doc
                .inclusive_descendants(doc.root())
                .into_iter()
                .filter(|&n| doc.element(n).is_some_and(|e| e.is_html("font")))
                .collect();
            doc.set_attribute(fonts[2], &"size".into(), "5".into())
                .unwrap();
        },
    };
    let p = parse_with(
        &mut host,
        "<p><font size=4><font size=4><font size=4><script>x</script><font size=4><p>X",
    );
    let expected = "| <html>\n|   <head>\n|   <body>\n|     <p>\n|       <font>\n|         size=\"4\"\n|         <font>\n|           size=\"4\"\n|           <font>\n|             size=\"5\"\n|             <script>\n|               \"x\"\n|             <font>\n|               size=\"4\"\n|     <p>\n|       <font>\n|         size=\"4\"\n|         <font>\n|           size=\"4\"\n|           <font>\n|             size=\"4\"\n|             \"X\"";
    assert_eq!(tree(&p), expected);
}

#[test]
fn scripted_foster01_equivalent() {
    // var t=document.querySelector('table');document.documentElement.remove();document.appendChild(t)
    for tail in ["<b>", "FOSTERTEXT"] {
        let mut host = FnHost {
            f: |p: &mut Parser, _| {
                let doc = p.document_mut();
                let t = doc
                    .inclusive_descendants(doc.root())
                    .into_iter()
                    .find(|&n| doc.element(n).is_some_and(|e| e.is_html("table")))
                    .unwrap();
                let html = doc.document_element().unwrap();
                doc.remove(html);
                let root = doc.root();
                doc.append(root, t);
            },
        };
        let p = parse_with(&mut host, &format!("<table><tr><script>x</script>{tail}"));
        assert_eq!(
            tree(&p),
            "| <table>\n|   <tbody>\n|     <tr>\n|       <script>\n|         \"x\"",
            "{tail}"
        );
    }
}

#[test]
fn dom_string_write_api() {
    // `write` acepta DomString con code points arbitrarios.
    let mut host = TestHost::new();
    let mut p = parse_with(&mut host, "");
    let s = DomString::from_code_points([0x3C, 0x69, 0x3E, 0x41]);
    p.write(&mut host, &s).unwrap();
    p.close(&mut host).unwrap();
    assert_eq!(text(p.document()), "A");
}

#[test]
fn load_waits_for_browser_resources() {
    let mut host = TestHost::new();
    let mut p = parser_for("<img src=a.png>", &opts());
    listen(&mut p, &mut host, EventTarget::Window, "load", "win");
    p.delay_load_event();
    p.finish(&mut host);
    // `DOMContentLoaded` ya pasó, pero `load` espera las imágenes.
    assert_eq!(p.status(), ParserStatus::Finishing);
    assert_eq!(p.document().ready_state(), DocumentReadiness::Interactive);
    assert!(host.log.is_empty());
    p.release_load_event(&mut host);
    assert_eq!(p.status(), ParserStatus::Complete);
    assert_eq!(host.log, ["win:load@document(complete)"]);
}
