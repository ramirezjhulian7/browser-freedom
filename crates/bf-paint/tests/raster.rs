//! Casos de pintura "pixel-exact" sin texto (TestFont no tiene contornos): fondos, bordes,
//! radios, recortes, orden de pintura, opacidad y visibilidad. Cada caso se compara con
//! `tests/expected/<nombre>.png`; `UPDATE_EXPECTED=1` los regenera (revisar a ojo después).

use std::path::PathBuf;

use bf_css::media::MediaContext;
use bf_css::selectors::MatchContext;
use bf_html::{ParseOptions, parse_document};
use bf_layout::{Size, TestFont, layout_document};
use bf_paint::{Pixmap, build_display_list, render};
use bf_style::style_document;
use bf_style::values::color::Rgba;

const VIEWPORT: Size = Size {
    width: 200.0,
    height: 150.0,
};

fn paint(html: &str) -> Pixmap {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let media = MediaContext {
        width: f64::from(VIEWPORT.width),
        height: f64::from(VIEWPORT.height),
        ..MediaContext::default()
    };
    let ctx = MatchContext::for_document(&doc);
    let styles = style_document(&doc, &[], &media, &ctx);
    let tree = layout_document(&doc, &styles, VIEWPORT, &TestFont);
    let dl = build_display_list(&doc, &tree, &TestFont);
    render(&dl, VIEWPORT, 1.0, Rgba::WHITE)
}

const CASES: &[(&str, &str)] = &[
    (
        "01-backgrounds",
        "<!DOCTYPE html><style>body{margin:10px;background:#ddd} div{height:20px}</style>
         <div style='background:red;width:100px'></div>
         <div style='background:lime;margin-left:20px;padding:5px'><div style='background:blue'></div></div>
         <div style='background:rgba(0,0,255,.5);width:50%'></div>",
    ),
    (
        "02-borders-solid",
        "<!DOCTYPE html><style>body{margin:10px}</style>
         <div style='width:60px;height:30px;border-style:solid;border-width:4px 8px 12px 16px;border-color:red lime blue orange'></div>
         <div style='width:30px;height:10px;margin-top:10px;border:5px solid black;background:yellow'></div>",
    ),
    (
        "03-border-styles",
        "<!DOCTYPE html><style>body{margin:5px} div{display:inline-block;width:20px;height:20px;margin:3px;border:6px #36c}</style>
         <div style='border-style:double'></div><div style='border-style:dashed'></div><div style='border-style:dotted'></div><div style='border-style:inset'></div><br>
         <div style='border-style:outset'></div><div style='border-style:groove'></div><div style='border-style:ridge'></div><div style='border-style:solid hidden none solid'></div>",
    ),
    (
        "04-border-radius",
        "<!DOCTYPE html><style>body{margin:10px}</style>
         <div style='width:80px;height:40px;background:teal;border-radius:20px'></div>
         <div style='width:60px;height:40px;margin-top:10px;border:6px solid maroon;border-radius:30px 0 / 20px 0;background:pink'></div>",
    ),
    (
        "05-overflow-clip",
        "<!DOCTYPE html><style>body{margin:10px}</style>
         <div style='width:60px;height:40px;overflow:hidden;border:3px solid black'><div style='width:100px;height:100px;background:orange'></div></div>
         <div style='width:60px;height:40px;margin-top:10px;overflow:hidden;border-radius:20px;background:#ccc'><div style='width:100px;height:100px;background:purple;margin:-10px'></div></div>",
    ),
    (
        "06-z-index",
        "<!DOCTYPE html><style>body{margin:0;position:relative;height:150px} div{position:absolute;width:60px;height:60px}</style>
         <div style='left:10px;top:10px;background:red;z-index:2'></div>
         <div style='left:30px;top:30px;background:lime'></div>
         <div style='left:50px;top:50px;background:blue;z-index:-1'></div>
         <div style='left:100px;top:10px;background:navy;z-index:1'><div style='left:20px;top:20px;background:yellow;z-index:-5'></div></div>",
    ),
    (
        "07-opacity",
        "<!DOCTYPE html><style>body{margin:10px}</style>
         <div style='opacity:.5;width:100px'><div style='height:40px;background:blue'></div><div style='height:40px;background:red;margin:-20px 20px 0'></div></div>
         <div style='position:relative;top:-10px;left:120px;width:40px;height:40px;background:green;opacity:0.25'></div>",
    ),
    (
        "08-paint-order",
        "<!DOCTYPE html><style>body{margin:10px}</style>
         <div style='float:left;width:40px;height:40px;background:red'></div>
         <div style='height:60px;background:lime'>
           <span style='display:inline-block;width:30px;height:30px;background:blue;vertical-align:top'></span>
         </div>
         <div style='margin-top:-20px;width:50px;height:30px;background:rgba(0,0,0,.5)'></div>",
    ),
    (
        "09-visibility",
        "<!DOCTYPE html><style>body{margin:10px}</style>
         <div style='visibility:hidden;background:red;padding:10px;border:3px solid red'><div style='visibility:visible;height:30px;background:green'></div></div>
         <div style='height:20px;background:navy;margin-top:5px'></div>",
    ),
    (
        "10-canvas",
        "<!DOCTYPE html><style>html{background:transparent} body{background:#9cf;margin:20px;height:40px;border:4px solid #036}</style><div style='width:30px;height:30px;background:white'></div>",
    ),
    (
        "11-form-checkbox-radio",
        "<!DOCTYPE html><style>body{margin:10px;font-size:10px} input{margin:4px}</style>
         <div><input type=checkbox><input type=checkbox checked><input type=radio><input type=radio checked></div>
         <div><input type=checkbox checked style='width:30px;height:30px'><input type=radio checked style='width:30px;height:30px'></div>",
    ),
    (
        "12-form-fields",
        "<!DOCTYPE html><style>body{margin:10px} input,select,textarea,button{font-size:10px;display:block;margin-bottom:6px}</style>
         <input size=10><textarea cols=12 rows=2></textarea>
         <select><option>abcdef</select><button style='width:60px;height:20px'></button>
         <input size=6 style='border:3px solid teal;background:#ffd;border-radius:6px'>",
    ),
    (
        "13-form-bars",
        "<!DOCTYPE html><style>body{margin:10px;font-size:10px} *{vertical-align:top}</style>
         <progress value=0.6></progress> <progress></progress><br>
         <meter value=0.9></meter> <meter value=0.5 low=0.6 high=0.8 optimum=0.9></meter> <meter value=0.1 low=0.3 optimum=0.9></meter><br>
         <input type=range value=25> <input type=color value='#3366cc' style='font-size:10px'>",
    ),
    (
        "14-fieldset-legend",
        "<!DOCTYPE html><style>body{margin:10px;font-size:10px} fieldset{margin:0 0 10px;border:4px solid navy;padding:6px} legend{width:40px;height:16px;background:orange;padding:0}</style>
         <fieldset><legend></legend><div style='height:20px;background:#9c9'></div></fieldset>
         <fieldset style='border:12px groove #c66'><legend style='height:6px;margin-left:10px'></legend><div style='height:10px;background:#99c'></div></fieldset>",
    ),
];

fn expected_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/expected")
}

#[test]
fn raster_cases() {
    let update = std::env::var_os("UPDATE_EXPECTED").is_some();
    let dir = expected_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut failures = Vec::new();
    for (name, html) in CASES {
        let actual = paint(html);
        let path = dir.join(format!("{name}.png"));
        if update {
            actual.save_png(&path).unwrap();
            continue;
        }
        let Ok(expected) = Pixmap::load_png(&path) else {
            failures.push(format!("{name}: falta {}", path.display()));
            continue;
        };
        if expected.data() != actual.data() {
            let out = std::env::temp_dir().join(format!("bf-paint-{name}-actual.png"));
            let _ = actual.save_png(&out);
            failures.push(format!("{name}: difiere (actual en {})", out.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(CASES.len(), 14);
}
