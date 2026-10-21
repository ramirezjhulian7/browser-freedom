//! CSSOM y CSSOM View (`bf_web::cssom`): `CSSStyleDeclaration`, `getComputedStyle`,
//! `document.styleSheets` y las reglas, `matchMedia` y la geometría. Sin shell, los estilos y
//! la maqueta se calculan bajo demanda (viewport 800×600, fuente de prueba).

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
        let url = Url::parse("https://example.com/dir/page.html", None).expect("url");
        let ctx = bf_web::install(
            &mut rt.agent,
            realm,
            WebContext::new(DocumentOwner::Document(Box::new(doc)), url),
        );
        let mut page = Page { rt, ctx };
        page.run(
            "var box = document.getElementById('box'), p = document.getElementById('p'),              s = document.getElementById('s'), c = document.getElementById('c'),              t = document.getElementById('t'), styleEl = document.head && document.head.firstChild;",
        );
        page
    }

    /// El resultado de `String(src)`.
    fn s(&mut self, src: &str) -> String {
        let realm = self.ctx.realm;
        let code = format!("String((function(){{ return ({src}); }})())");
        match self.rt.eval_script(realm, &code, "test.js") {
            Ok(v) => self.rt.convert::<String>(&v).expect("string"),
            Err(e) => panic!("error en {src}: {e}\n{}", e.stack),
        }
    }

    fn run(&mut self, src: &str) {
        let realm = self.ctx.realm;
        if let Err(e) = self.rt.eval_script(realm, src, "test.js") {
            panic!("error: {e}\n{}", e.stack);
        }
    }

    /// El nombre del error que lanza `src`.
    fn error(&mut self, src: &str) -> String {
        let code = format!(
            "(function(){{ try {{ {src}; return 'sin error'; }} catch (e) {{ return e.name; }} }})()"
        );
        self.s(&code)
    }
}

const BOX: &str = "<!DOCTYPE html><html><head><style>\
    body { margin: 0 }\
    #box { width: 100px; height: 50px; padding: 5px; border: 2px solid red; margin: 10px }\
    .blue { color: blue }\
    @media (min-width: 500px) { .wide { color: green } }\
    </style></head><body><div id=box class=blue></div><p id=p style='color: red; margin-top: 3px'>x</p>\
    <span id=s>t</span></body></html>";

fn page() -> Page {
    Page::new(BOX)
}

macro_rules! case {
    ($name:ident, $js:expr, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(page().s($js), $expected);
        }
    };
    ($name:ident, html $html:expr, $js:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new($html);
            assert_eq!(p.s($js), $expected);
        }
    };
    ($name:ident, html $html:expr, setup $setup:expr, $js:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = Page::new($html);
            p.run($setup);
            assert_eq!(p.s($js), $expected);
        }
    };
    ($name:ident, setup $setup:expr, $js:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let mut p = page();
            p.run($setup);
            assert_eq!(p.s($js), $expected);
        }
    };
}

// --- CSSStyleDeclaration sobre el atributo style (CSSOM §6.7) ---

case!(
    style_is_declaration,
    "p.style instanceof CSSStyleDeclaration",
    "true"
);
case!(style_same_object, "p.style === p.style", "true");
case!(style_reads_attribute, "p.style.color", "red");
case!(
    style_get_property_value,
    "p.style.getPropertyValue('margin-top')",
    "3px"
);
case!(style_length, "p.style.length", "2");
case!(
    style_item,
    "p.style.item(0) + ',' + p.style.item(1) + ',' + p.style.item(9)",
    "color,margin-top,"
);
case!(style_indexed, "p.style[1]", "margin-top");
case!(
    style_css_text,
    "p.style.cssText",
    "color: red; margin-top: 3px;"
);
case!(style_set_camel, setup "box.style.backgroundColor = 'Blue'", "box.getAttribute('style')", "background-color: blue;");
case!(style_set_dashed, setup "box.style['background-color'] = 'red'", "box.style.backgroundColor", "red");
case!(style_set_property, setup "box.style.setProperty('width', '10.50PX')", "box.style.width", "10.5px");
case!(style_set_invalid_ignored, setup "box.style.width = '10px'; box.style.width = 'banana'", "box.style.width", "10px");
case!(style_set_empty_removes, setup "p.style.color = ''", "p.style.cssText", "margin-top: 3px;");
case!(
    style_remove_property,
    "p.style.removeProperty('color') + '|' + p.style.cssText",
    "red|margin-top: 3px;"
);
case!(style_priority, setup "box.style.setProperty('color', 'red', 'important')", "box.style.getPropertyPriority('color') + '|' + box.style.cssText", "important|color: red !important;");
case!(style_bad_priority_ignored, setup "box.style.setProperty('color', 'red', 'urgent')", "box.style.cssText", "");
case!(style_unknown_property_ignored, setup "box.style.setProperty('colour', 'red')", "box.style.length", "0");
case!(style_shorthand_expands, setup "box.style.margin = '1px 2px'", "box.style.marginLeft + ' ' + box.style.marginTop", "2px 1px");
case!(style_shorthand_serializes, setup "box.style.margin = '1px 2px 3px'", "box.style.margin", "1px 2px 3px");
case!(style_shorthand_in_css_text, setup "box.style.padding = '4px'", "box.style.cssText", "padding: 4px;");
case!(style_partial_shorthand_empty, setup "box.style.marginTop = '1px'", "box.style.margin", "");
case!(style_border_shorthand, setup "box.style.border = '1px solid'", "box.style.borderTopStyle + '|' + box.style.border", "solid|1px solid");
case!(style_custom_property, setup "box.style.setProperty('--Main', ' 10px  20px ')", "box.style.getPropertyValue('--Main')", "10px  20px");
case!(style_css_float, setup "box.style.cssFloat = 'left'", "box.style.float + box.style.cssFloat", "leftleft");
case!(style_css_text_setter, setup "box.style.cssText = 'color: red; width: 1px; bogus: 1'", "box.style.length + '|' + box.style.width", "2|1px");
case!(style_put_forwards, setup "box.style = 'height: 7px'", "box.style.height + '|' + box.getAttribute('style')", "7px|height: 7px;");
case!(style_tracks_attribute, setup "box.setAttribute('style', 'top: 1px')", "box.style.top", "1px");
case!(style_hex_color, setup "box.style.color = '#00F'", "box.style.color", "rgb(0, 0, 255)");
case!(style_null_to_empty, setup "p.style.color = null", "p.style.color", "");
case!(style_parent_rule_null, "p.style.parentRule", "null");
case!(
    style_has_camel_attributes,
    "'borderTopLeftRadius' in document.body.style && 'webkitBoxShadow' in CSSStyleDeclaration.prototype === false",
    "true"
);
case!(
    style_important_parsing,
    "(function(){ box.setAttribute('style', 'color: red !important; color: blue'); return box.style.color; })()",
    "red"
);

// --- getComputedStyle (CSSOM §9) ---

case!(
    computed_color,
    "getComputedStyle(p).color",
    "rgb(255, 0, 0)"
);
case!(
    computed_from_sheet,
    "getComputedStyle(box).color",
    "rgb(0, 0, 255)"
);
case!(
    computed_display,
    "getComputedStyle(box).display + ' ' + getComputedStyle(s).display",
    "block inline"
);
case!(computed_width_px, "getComputedStyle(box).width", "100px");
case!(computed_margin, "getComputedStyle(box).marginLeft", "10px");
case!(
    computed_border,
    "getComputedStyle(box).borderTopWidth + ' ' + getComputedStyle(box).borderTopStyle",
    "2px solid"
);
case!(computed_shorthand, "getComputedStyle(box).padding", "5px");
case!(
    computed_readonly,
    "(function(){ try { getComputedStyle(box).color = 'red'; } catch (e) {} return getComputedStyle(box).color; })()",
    "rgb(0, 0, 255)"
);
case!(
    computed_set_property_throws,
    "(function(){ try { getComputedStyle(box).setProperty('color','red'); } catch (e) { return e.name; } })()",
    "NoModificationAllowedError"
);
case!(computed_length, "getComputedStyle(box).length > 50", "true");
case!(
    computed_item_sorted,
    "(function(){ var c = getComputedStyle(box); return c.item(0) < c.item(1); })()",
    "true"
);
case!(computed_css_text_empty, "getComputedStyle(box).cssText", "");
case!(computed_live, setup "box.style.color = 'lime'", "getComputedStyle(box).color", "rgb(0, 255, 0)");
case!(computed_inherits, setup "document.body.style.color = 'red'", "getComputedStyle(s).color", "rgb(255, 0, 0)");
case!(
    computed_requires_element,
    "(function(){ try { getComputedStyle(document); } catch (e) { return e.name; } })()",
    "TypeError"
);
case!(
    computed_pseudo_none,
    "getComputedStyle(box, '::before').display",
    ""
);
case!(computed_display_none_width, setup "box.style.display = 'none'", "getComputedStyle(box).width", "100px");

// --- Hojas y reglas (CSSOM §6.1–§6.4) ---

case!(sheets_length, "document.styleSheets.length", "1");
case!(
    sheets_indexed,
    "document.styleSheets[0] instanceof CSSStyleSheet",
    "true"
);
case!(
    sheets_owner_node,
    "document.styleSheets[0].ownerNode === styleEl",
    "true"
);
case!(
    sheets_type_href,
    "document.styleSheets[0].type + ' ' + document.styleSheets[0].href",
    "text/css null"
);
case!(
    sheets_identity,
    "document.styleSheets[0] === styleEl.sheet",
    "true"
);
case!(rules_length, "document.styleSheets[0].cssRules.length", "4");
case!(
    rule_selector_text,
    "document.styleSheets[0].cssRules[1].selectorText",
    "#box"
);
case!(
    rule_type,
    "document.styleSheets[0].cssRules[1].type + ' ' + document.styleSheets[0].cssRules[3].type",
    "1 4"
);
case!(
    rule_css_text,
    "document.styleSheets[0].cssRules[2].cssText",
    ".blue { color: blue; }"
);
case!(
    rule_style,
    "document.styleSheets[0].cssRules[1].style.width",
    "100px"
);
case!(
    rule_media,
    "document.styleSheets[0].cssRules[3].media.mediaText + '|' + document.styleSheets[0].cssRules[3].conditionText",
    "(min-width: 500px)|(min-width: 500px)"
);
case!(
    rule_media_children,
    "document.styleSheets[0].cssRules[3].cssRules[0].selectorText",
    ".wide"
);
case!(
    rule_parent_rule,
    "(function(){ var m = document.styleSheets[0].cssRules[3]; return m.cssRules[0].parentRule === m; })()",
    "true"
);
case!(
    rule_parent_sheet,
    "document.styleSheets[0].cssRules[0].parentStyleSheet === document.styleSheets[0]",
    "true"
);
case!(
    rule_identity,
    "document.styleSheets[0].cssRules[1] === document.styleSheets[0].cssRules[1]",
    "true"
);
case!(insert_rule_applies, setup "document.styleSheets[0].insertRule('#s { color: lime }', 4)", "getComputedStyle(s).color", "rgb(0, 255, 0)");
case!(
    insert_rule_returns_index,
    "document.styleSheets[0].insertRule('p { color: red }')",
    "0"
);
case!(
    insert_rule_syntax_error,
    "(function(){ try { document.styleSheets[0].insertRule('}{'); } catch (e) { return e.name; } })()",
    "SyntaxError"
);
case!(
    insert_rule_index_error,
    "(function(){ try { document.styleSheets[0].insertRule('p {}', 99); } catch (e) { return e.name; } })()",
    "IndexSizeError"
);
case!(
    insert_import_hierarchy,
    "(function(){ try { document.styleSheets[0].insertRule('@import url(x.css);', 1); } catch (e) { return e.name; } })()",
    "HierarchyRequestError"
);
case!(delete_rule, setup "document.styleSheets[0].deleteRule(2)", "getComputedStyle(box).color + ' ' + document.styleSheets[0].cssRules.length", "rgb(0, 0, 0) 3");
case!(
    rule_list_live,
    "(function(){ var l = document.styleSheets[0].cssRules; document.styleSheets[0].insertRule('a {}', 0); return l.length + ' ' + l[0].selectorText; })()",
    "5 a"
);
case!(rule_style_mutation_applies, setup "document.styleSheets[0].cssRules[2].style.color = 'red'", "getComputedStyle(box).color", "rgb(255, 0, 0)");
case!(selector_text_setter, setup "document.styleSheets[0].cssRules[2].selectorText = '#s'", "getComputedStyle(s).color + ' ' + getComputedStyle(box).color", "rgb(0, 0, 255) rgb(0, 0, 0)");
case!(sheet_disabled, setup "document.styleSheets[0].disabled = true", "getComputedStyle(box).paddingLeft + ' ' + getComputedStyle(box).width", "0px 784px");
case!(
    style_text_change_new_sheet,
    "(function(){ var a = document.styleSheets[0]; styleEl.textContent = 'p { color: red }'; var b = document.styleSheets[0]; return (a !== b) + ' ' + b.cssRules.length + ' ' + (a.ownerNode === null); })()",
    "true 1 true"
);
case!(
    constructed_sheet,
    "(function(){ var s = new CSSStyleSheet(); s.replaceSync('a { color: red } b { color: blue }'); return s.cssRules.length + ' ' + s.cssRules[1].cssText; })()",
    "2 b { color: blue; }"
);
case!(
    media_list_append,
    "(function(){ var s = new CSSStyleSheet({media: 'screen'}); s.media.appendMedium('print'); return s.media.mediaText + ' ' + s.media.length + ' ' + s.media[1]; })()",
    "screen, print 2 print"
);
case!(
    keyframes_rule,
    "(function(){ var s = new CSSStyleSheet(); s.replaceSync('@keyframes k { from { color: red } to { color: blue } }'); var k = s.cssRules[0]; return k.name + ' ' + k.cssRules.length + ' ' + k.cssRules[0].keyText + ' ' + k.findRule('to').style.color; })()",
    "k 2 0% blue"
);
case!(
    rule_constants,
    "CSSRule.STYLE_RULE + CSSRule.MEDIA_RULE + CSSRule.prototype.SUPPORTS_RULE",
    "17"
);

// --- matchMedia (CSSOM View §4.2) ---

case!(
    match_media_true,
    "matchMedia('(min-width: 500px)').matches",
    "true"
);
case!(
    match_media_false,
    "matchMedia('(max-width: 500px)').matches",
    "false"
);
case!(
    match_media_text,
    "matchMedia('(MIN-WIDTH:500px)').media",
    "(min-width: 500px)"
);
case!(
    match_media_invalid,
    "matchMedia('screen and and').media",
    "not all"
);
case!(
    match_media_class,
    "matchMedia('all') instanceof MediaQueryList && matchMedia('all') instanceof EventTarget",
    "true"
);
case!(media_rule_applies, setup "s.className = 'wide'", "getComputedStyle(s).color", "rgb(0, 128, 0)");

// --- Geometría (CSSOM View §6, §7) ---

case!(
    offset_width,
    "box.offsetWidth + 'x' + box.offsetHeight",
    "114x64"
);
case!(
    client_width,
    "box.clientWidth + 'x' + box.clientHeight + ' ' + box.clientTop",
    "110x60 2"
);
case!(
    offset_position,
    "box.offsetLeft + ',' + box.offsetTop",
    "10,10"
);
case!(
    offset_parent_body,
    "box.offsetParent === document.body",
    "true"
);
case!(offset_parent_positioned, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div id=o style=\"position:relative; margin-left:20px\"><div id=i style=\"margin-left:5px; width:10px; height:10px\"></div></div></body></html>", "(function(){ var i = document.getElementById('i'); return (i.offsetParent.id) + ' ' + i.offsetLeft; })()", "o 5");
case!(
    bounding_rect,
    "(function(){ var r = box.getBoundingClientRect(); return [r.x, r.y, r.width, r.height, r.right, r.bottom].join(); })()",
    "10,10,114,64,124,74"
);
case!(
    bounding_rect_class,
    "box.getBoundingClientRect() instanceof DOMRect && box.getBoundingClientRect() instanceof DOMRectReadOnly",
    "true"
);
case!(
    client_rects,
    "box.getClientRects().length + ' ' + box.getClientRects()[0].width",
    "1 114"
);
case!(no_box_zero, setup "box.style.display = 'none'", "box.offsetWidth + ' ' + box.getClientRects().length + ' ' + box.getBoundingClientRect().width + ' ' + box.offsetParent", "0 0 0 null");
case!(geometry_follows_style, setup "box.style.width = '200px'", "box.offsetWidth", "214");
case!(
    element_from_point,
    "document.elementFromPoint(50, 30).id",
    "box"
);
case!(
    element_from_point_outside,
    "document.elementFromPoint(-1, 5)",
    "null"
);
case!(
    elements_from_point,
    "document.elementsFromPoint(50, 30).map(function(e){ return e.localName; }).join()",
    "div,body,html"
);
case!(check_visibility, setup "s.style.visibility = 'hidden'", "box.checkVisibility() + ' ' + s.checkVisibility() + ' ' + s.checkVisibility({visibilityProperty: true})", "true true false");
case!(check_visibility_none, setup "document.body.style.display = 'none'", "box.checkVisibility()", "false");
case!(scroll_size, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div id=c style=\"overflow:scroll; width:50px; height:50px\"><div style=\"width:200px; height:300px\"></div></div></body></html>", "(function(){ var c = document.getElementById('c'); return c.scrollWidth + 'x' + c.scrollHeight; })()", "200x300");
case!(element_scroll_top, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div id=c style=\"overflow:scroll; width:50px; height:50px\"><div style=\"height:300px\"></div></div></body></html>", "(function(){ var c = document.getElementById('c'); c.scrollTop = 1000; var a = c.scrollTop; c.scrollTo(0, 20); return a + ' ' + c.scrollTop; })()", "250 20");

// --- DOMRect (Geometry Interfaces §4) ---

case!(
    dom_rect_ctor,
    "(function(){ var r = new DOMRect(1, 2, -3, 4); return [r.left, r.right, r.top, r.bottom].join(); })()",
    "-2,1,2,6"
);
case!(
    dom_rect_mutable,
    "(function(){ var r = new DOMRect(); r.width = 5; return r.width + ' ' + JSON.stringify(r.toJSON()); })()",
    "5 {\"x\":0,\"y\":0,\"width\":5,\"height\":0,\"top\":0,\"right\":5,\"bottom\":0,\"left\":0}"
);
case!(
    dom_rect_read_only,
    "(function(){ var r = new DOMRectReadOnly(1, 1, 1, 1); r.x = 9; return r.x; })()",
    "1"
);
case!(
    dom_rect_from_rect,
    "DOMRect.fromRect({x: 3, width: 2}).right",
    "5"
);

// --- Window (CSSOM View §4) ---

case!(
    window_inner_size,
    "innerWidth + 'x' + innerHeight",
    "800x600"
);
case!(window_dpr, "devicePixelRatio + ' ' + outerWidth", "1 800");
case!(
    window_scroll_initial,
    "scrollX + ',' + scrollY + ',' + pageYOffset",
    "0,0,0"
);
case!(window_scroll_to, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div style=\"height:2000px\"></div></body></html>", setup "scrollTo(0, 300)", "scrollY + ' ' + document.body.firstChild.getBoundingClientRect().top", "300 -300");
case!(window_scroll_clamped, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div style=\"height:1000px\"></div></body></html>", setup "scrollTo({top: 5000})", "scrollY", "400");
case!(window_scroll_by, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div style=\"height:2000px\"></div></body></html>", setup "scrollBy(0, 10); scrollBy({top: 15})", "scrollY", "25");
case!(scroll_into_view, html "<!DOCTYPE html><html><head><style>body { margin: 0 }</style></head><body><div style=\"height:1500px\"></div><p id=t style=\"height:10px; margin:0\">x</p><div style=\"height:1500px\"></div></body></html>", setup "document.getElementById('t').scrollIntoView()", "scrollY", "1500");
case!(
    scrolling_element,
    "document.scrollingElement === document.documentElement",
    "true"
);

#[test]
fn errors_are_dom_exceptions() {
    let mut p = page();
    assert_eq!(
        p.error("document.styleSheets[0].deleteRule(10)"),
        "IndexSizeError"
    );
    assert_eq!(
        p.error("new CSSStyleSheet().media.deleteMedium('print')"),
        "NotFoundError"
    );
}

#[test]
fn snapshot_from_the_shell_is_used() {
    use std::sync::Arc;
    let mut p = page();
    // Un snapshot "del shell" con un viewport distinto y la versión actual del documento.
    let realm = p.ctx.realm;
    let snapshot = {
        let ctx = p.ctx.get(&p.rt.agent);
        let doc = ctx.document();
        let styles = bf_style::style_document(
            doc,
            &[],
            &bf_css::media::MediaContext::default(),
            &bf_css::selectors::MatchContext::for_document(doc),
        );
        let tree = bf_layout::layout_document(
            doc,
            &styles,
            bf_layout::Size {
                width: 1000.0,
                height: 700.0,
            },
            &bf_layout::TestFont,
        );
        let mut s = bf_web::LayoutSnapshot::new(Arc::new(styles), Arc::new(tree), doc.version());
        s.device_pixel_ratio = 2.0;
        s.scroll = (0.0, 0.0);
        s
    };
    p.ctx.get_mut(&mut p.rt.agent).layout = Some(snapshot);
    let _ = realm;
    assert_eq!(p.s("innerWidth + ' ' + devicePixelRatio"), "1000 2");
    assert_eq!(p.s("box.offsetWidth"), "114");
}
