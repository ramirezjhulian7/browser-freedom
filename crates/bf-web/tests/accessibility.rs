//! E1: el árbol de accesibilidad de documentos representativos, con el estilo de la página
//! (`bf_web::html::accessibility::accessibility_tree_text`).

use bf_js::host::DefaultHooks;
use bf_web::{DocumentOwner, Runtime, Url, WebContext, WebContextHandle};

struct Page {
    rt: Runtime,
    ctx: WebContextHandle,
}

impl Page {
    fn new(html: &str) -> Page {
        let mut rt = Runtime::new(Box::new(DefaultHooks));
        let (doc, _) = bf_html::parse_document(html, &bf_html::ParseOptions::default());
        let realm = rt.main_realm();
        let url = Url::parse("https://example.com/wiki/Articulo", None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        Page { rt, ctx }
    }

    fn eval(&mut self, src: &str) {
        let realm = self.ctx.realm;
        if let Err(e) = self.rt.eval_script(realm, src, "test.js") {
            panic!("error: {e}\n{}", e.stack);
        }
    }

    fn tree(&mut self) -> String {
        bf_web::html::accessibility::accessibility_tree_text(&mut self.rt.agent, 0)
    }
}

/// Un documento con encabezados, listas, tabla, formulario, enlaces, imágenes y lo que se oculta
/// de las tres formas (`aria-hidden`, `display: none`, `visibility: hidden`).
const REPRESENTATIVE: &str = "<!DOCTYPE html><html lang=es><head><title>Página de prueba</title>\
<style>.gone { display: none } .inv { visibility: hidden } .vis { visibility: visible }</style>\
<script>var x = 1;</script></head><body>\
<header><nav aria-label=Principal><ul><li><a href=/>Inicio</a></li><li><a href=/b>Buscar</a></li></ul></nav></header>\
<main><h1>Título</h1>\
<p>Un párrafo con <a href=#x>enlace</a> y <strong>negrita</strong>.</p>\
<img src=a.png alt='Un gato'><img src=b.png alt=''>\
<ol><li>uno</li><li>dos</li></ol>\
<table><caption>Datos</caption><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>\
<form aria-label=Contacto><label for=n>Nombre</label><input id=n value=Ana>\
<label><input type=checkbox checked> Acepto</label>\
<input type=radio name=r> <button aria-pressed=true>Negrita</button>\
<select><option>Uno<option selected>Dos</select><button disabled>Enviar</button></form>\
<div aria-hidden=true><p>secreto</p></div><div class=gone>tampoco</div>\
<div class=inv>no <span class=vis>sí</span></div>\
<details open><summary>Más</summary>contenido</details>\
<h2 aria-describedby=d>Sub</h2><p id=d>desc</p>\
<div role=tablist aria-owns=t2><div role=tab aria-selected=true>T1</div></div><div id=t2 role=tab>T2</div>\
</main></body></html>";

#[test]
fn tree_of_representative_document() {
    let mut p = Page::new(REPRESENTATIVE);
    let expected = r#"document "Página de prueba"
  banner
    navigation "Principal"
      list
        listitem
          link "Inicio"
            text "Inicio"
        listitem
          link "Buscar"
            text "Buscar"
  main
    heading "Título" level=1
      text "Título"
    paragraph
      text "Un párrafo con"
      link "enlace"
        text "enlace"
      text "y"
      strong
        text "negrita"
      text "."
    image "Un gato"
    list
      listitem
        text "uno"
      listitem
        text "dos"
    table "Datos"
      caption
        text "Datos"
      rowgroup
        row "A B"
          columnheader "A"
            text "A"
          columnheader "B"
            text "B"
        row "1 2"
          cell "1"
            text "1"
          cell "2"
            text "2"
    form "Contacto"
      text "Nombre"
      textbox "Nombre" value="Ana"
      checkbox "Acepto" checked=true
      text "Acepto"
      radio checked=false
      button "Negrita" pressed=true
      combobox value="Dos"
        option "Uno" selected=false
        option "Dos" selected=true
      button "Enviar" disabled
    text "sí"
    group
      button "Más" expanded=true
      text "contenido"
    heading "Sub" level=2 description="desc"
      text "Sub"
    paragraph
      text "desc"
    tablist
      tab "T1" selected=true
      tab "T2"
"#;
    assert_eq!(p.tree(), expected);
}

#[test]
fn tree_follows_control_state() {
    let mut p = Page::new(
        "<!DOCTYPE html><body><input id=c type=checkbox aria-label=Todo>\
         <input id=t aria-label=Ciudad value=Lima>\
         <details id=d><summary>Detalles</summary>oculto</details>",
    );
    let before = p.tree();
    assert!(
        before.contains("checkbox \"Todo\" checked=false"),
        "{before}"
    );
    assert!(
        before.contains("textbox \"Ciudad\" value=\"Lima\""),
        "{before}"
    );
    assert!(
        before.contains("button \"Detalles\" expanded=false"),
        "{before}"
    );
    p.eval(
        "document.getElementById('c').indeterminate = true;\
         document.getElementById('t').value = 'Quito';\
         document.getElementById('d').open = true;",
    );
    let after = p.tree();
    assert!(after.contains("checkbox \"Todo\" checked=mixed"), "{after}");
    assert!(
        after.contains("textbox \"Ciudad\" value=\"Quito\""),
        "{after}"
    );
    assert!(
        after.contains("button \"Detalles\" expanded=true"),
        "{after}"
    );
}

/// La estructura de un artículo de Wikipedia (piel Vector): salto al contenido, cabecera con
/// logo y búsqueda, menú lateral, título, ficha (tabla flotante), índice, secciones con su
/// enlace de edición oculto, referencias y pie.
const WIKIPEDIA: &str = include_str!("a11y_wikipedia.html");

#[test]
fn wikipedia_article_reading_order() {
    let mut p = Page::new(WIKIPEDIA);
    let tree = p.tree();
    // El orden de lectura de las marcas, los encabezados y los bloques: el del DOM (la ficha
    // flota a la derecha, pero se lee donde está en el documento).
    let outline: Vec<&str> = tree
        .lines()
        .map(str::trim)
        .filter(|l| {
            [
                "banner",
                "search",
                "navigation",
                "main",
                "heading",
                "table",
                "contentinfo",
            ]
            .iter()
            .any(|r| l.split(' ').next() == Some(*r))
        })
        .collect();
    assert_eq!(
        outline,
        [
            "banner",
            "search \"Buscar en Wikipedia\"",
            "navigation \"Sitio\"",
            "heading \"Navegación\" level=3",
            "main",
            "heading \"Gato\" level=1",
            "table \"Gato doméstico\"",
            "navigation \"Índice\"",
            "heading \"Índice\" level=2",
            "heading \"Historia\" level=2",
            "heading \"Referencias\" level=2",
            "contentinfo",
        ],
        "{tree}"
    );
    // Lo primero es el salto al contenido; lo oculto (`display: none`, el `[editar]` con
    // `aria-hidden`) no se lee; la referencia es un enlace dentro del párrafo.
    assert!(
        tree.starts_with(
            "document \"Gato - Wikipedia, la enciclopedia libre\"\n  link \"Ir al contenido\"\n"
        ),
        "{tree}"
    );
    assert!(!tree.contains("Solo para imprimir"), "{tree}");
    assert!(!tree.contains("editar"), "{tree}");
    assert!(
        tree.contains("      superscript\n        link \"[1]\"\n"),
        "{tree}"
    );
    assert!(tree.contains("link \"1 Historia\""), "{tree}");
    assert!(tree.contains("rowheader \"Reino\""), "{tree}");
    assert!(tree.contains("link \"Volver arriba\""), "{tree}");
}
