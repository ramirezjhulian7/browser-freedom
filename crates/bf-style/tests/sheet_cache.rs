//! E4: la caché de hojas parseadas (`bf_style::sheet_cache`): una hoja ya parseada no se
//! vuelve a parsear al cargarla, y la clave distingue URL, bytes y encoding de respaldo.

use std::sync::Arc;

use bf_css::media::MediaContext;
use bf_css::stylesheet::Stylesheet;
use bf_html::{ParseOptions, parse_document};
use bf_infra::encoding::Encoding;
use bf_infra::{Resource, ResourceLoader, Url};
use bf_style::sheet_cache::{self, SheetContext, SheetKey};
use bf_style::{Resources, document_stylesheets};

struct Hojas(Vec<(&'static str, &'static str)>);

impl ResourceLoader for Hojas {
    fn load(&self, url: &Url) -> Option<Resource> {
        let href = url.href();
        let (_, css) = self.0.iter().find(|(u, _)| href == *u)?;
        Some(Resource {
            bytes: css.as_bytes().to_vec(),
            url: url.clone(),
            content_type: Some("text/css".to_owned()),
        })
    }
}

fn assert_send_sync<T: Send + Sync>() {}

// Un solo test: las cuentas de la caché son del proceso.
#[test]
fn cached_sheets_are_not_parsed_again() {
    assert_send_sync::<Arc<Stylesheet>>();
    let ctx = SheetContext {
        quirks: false,
        encoding: Encoding::Utf8,
    };
    let url = Url::parse("https://cache.test/a.css", None).unwrap();

    // `warm` parsea una vez; `get` y un segundo `warm` dan la misma hoja sin parsear.
    sheet_cache::take_stats();
    let first = sheet_cache::warm(&url, b"p { color: red } a { background: url(i.png) }", &ctx);
    let key = SheetKey::new(&url, b"p { color: red } a { background: url(i.png) }");
    let again = sheet_cache::get(&key, &ctx).expect("en la caché");
    assert!(Arc::ptr_eq(&first, &again));
    let st = sheet_cache::take_stats();
    assert_eq!((st.parsed, st.hits), (1, 1));
    // Las `url()` quedan resueltas contra la URL de la hoja.
    assert!(format!("{first:?}").contains("https://cache.test/i.png"));

    // Otros bytes, otra URL u otro encoding: otra entrada.
    assert!(sheet_cache::get(&SheetKey::new(&url, b"p { color: blue }"), &ctx).is_none());
    let other = Url::parse("https://cache.test/b.css", None).unwrap();
    assert!(
        sheet_cache::get(
            &SheetKey::new(&other, b"p { color: red } a { background: url(i.png) }"),
            &ctx
        )
        .is_none()
    );
    let latin = SheetContext {
        encoding: Encoding::for_label("windows-1252").expect("windows-1252"),
        ..ctx
    };
    assert!(sheet_cache::get(&key, &latin).is_none());

    // Cargar el documento: la primera vez se parsea, la segunda sale de la caché, y las dos
    // hojas resultantes son iguales.
    let (doc, _) = parse_document(
        "<!doctype html><link rel=stylesheet href=c.css><p>x",
        &ParseOptions::default(),
    );
    let base = Url::parse("https://cache.test/doc.html", None).unwrap();
    let loader = Hojas(vec![
        (
            "https://cache.test/c.css",
            "@import url(d.css); .x { color: green }",
        ),
        ("https://cache.test/d.css", ".y { color: navy }"),
    ]);
    let res = Resources {
        loader: &loader,
        base: &base,
        encoding: Encoding::Utf8,
    };
    sheet_cache::take_stats();
    let one = document_stylesheets(&doc, &MediaContext::default(), Some(res));
    let st = sheet_cache::take_stats();
    assert_eq!((st.parsed, st.hits), (2, 0), "c.css y la importada d.css");
    let two = document_stylesheets(&doc, &MediaContext::default(), Some(res));
    let st = sheet_cache::take_stats();
    assert_eq!((st.parsed, st.hits), (0, 2), "las dos de la caché");
    assert_eq!(one, two);
}
