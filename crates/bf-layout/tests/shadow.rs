//! El layout del flat tree (CSS Scoping §2.2, E1): el shadow tree de un host reemplaza a sus
//! hijos, los nodos asignados se maquetan en su `<slot>` y los no asignados no se ven.

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, dump, layout_document};
use bf_style::style_document;

fn layout(html: &str) -> String {
    let opts = ParseOptions {
        allow_declarative_shadow_roots: true,
        ..ParseOptions::default()
    };
    let (doc, _) = parse_document(html, &opts);
    let ctx = MatchContext::for_document(&doc);
    let media = MediaContext {
        width: 800.0,
        height: 600.0,
        ..MediaContext::default()
    };
    let styles = style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(
        &doc,
        &styles,
        Size {
            width: 800.0,
            height: 600.0,
        },
        &TestFont,
    );
    dump(&doc, &tree)
}

#[test]
fn slotted_content_is_laid_out_in_the_slot() {
    let out = layout(
        "<!DOCTYPE html><body style='margin:0'><div id=host>\
         <template shadowrootmode=open><p>antes</p><slot name=a></slot><p>despues</p></template>\
         <span slot=a>asignado</span><span>sin-slot</span></div>",
    );
    let before = out.find("antes").expect("contenido del shadow tree");
    let slotted = out.find("asignado").expect("nodo asignado");
    let after = out.find("despues").expect("contenido del shadow tree");
    assert!(before < slotted && slotted < after, "{out}");
    assert!(!out.contains("sin-slot"), "{out}");
}

#[test]
fn slot_fallback_content() {
    let out = layout(
        "<!DOCTYPE html><div><template shadowrootmode=open><slot>respaldo</slot></template></div>\
         <div><template shadowrootmode=open><slot>oculto</slot></template>luz</div>",
    );
    assert!(out.contains("respaldo"), "{out}");
    assert!(out.contains("luz") && !out.contains("oculto"), "{out}");
}
