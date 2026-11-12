//! CSS moderno en valores: `color-mix()` y los espacios de color de CSS Color 4, `env()`,
//! `@property`, las math functions de CSS Values 4 §10 y las unidades de viewport dinámicas.

use bf_css::media::MediaContext;
use bf_css::selectors::{MatchContext, NamespaceMap, matches, parse_selector_list};
use bf_html::{ParseOptions, parse_document};
use bf_style::style_document;

/// El valor computado de `property` en el primer elemento que cumple `selector`.
fn computed(html: &str, selector: &str, property: &str) -> String {
    let (doc, _) = parse_document(html, &ParseOptions::default());
    let ctx = MatchContext::for_document(&doc);
    let map = style_document(&doc, &[], &MediaContext::default(), &ctx);
    let list = parse_selector_list(selector, &NamespaceMap::default()).expect("selector");
    let node = doc
        .inclusive_descendants(doc.root())
        .into_iter()
        .find(|&n| list.0.iter().any(|s| matches(s, &doc, n, &ctx)))
        .unwrap_or_else(|| panic!("ningún elemento coincide con {selector}"));
    map[&(node, None)]
        .get_css(property)
        .unwrap_or_else(|| panic!("propiedad desconocida {property}"))
}

/// `color` de un `<p>` con `style="color: <value>"`.
fn color(value: &str) -> String {
    computed(
        &format!("<!DOCTYPE html><p style=\"color: {value}\">x</p>"),
        "p",
        "color",
    )
}

/// Ejecuta casos (valor, esperado) y junta los fallos.
fn check(cases: &[(&str, &str)], f: impl Fn(&str) -> String) {
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(v, want)| {
            let got = f(v);
            (got != *want).then(|| format!("{v}: {got} (esperado {want})"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn color_spaces() {
    check(
        &[
            // CSS Color 4 §9: lab/lch/oklab/oklch (ejemplos de la spec, redondeados).
            ("lab(54.29 80.8 69.89)", "rgb(255, 0, 0)"),
            ("lch(54.29 106.84 40.85)", "rgb(255, 0, 0)"),
            ("oklab(0.628 0.2249 0.1258)", "rgb(255, 0, 0)"),
            ("oklch(0.628 0.2577 29.23)", "rgb(255, 0, 0)"),
            ("oklch(100% 0 0)", "rgb(255, 255, 255)"),
            ("lab(0 0 0)", "rgb(0, 0, 0)"),
            ("oklab(50% 0 0 / 0.5)", "rgba(99, 99, 99, 0.5)"),
            ("lch(50% 0 none)", "rgb(119, 119, 119)"),
            // §10: color() con espacios predefinidos.
            ("color(srgb 1 0 0)", "rgb(255, 0, 0)"),
            ("color(srgb 100% 50% 0%)", "rgb(255, 128, 0)"),
            ("color(srgb-linear 0.2 0.2 0.2)", "rgb(124, 124, 124)"),
            ("color(display-p3 0 0 0 / 50%)", "rgba(0, 0, 0, 0.5)"),
            ("color(xyz 0.9505 1 1.089)", "rgb(255, 255, 255)"),
            ("color(xyz-d50 0.9642 1 0.8252)", "rgb(255, 255, 255)"),
            ("color(rec2020 1 1 1)", "rgb(255, 255, 255)"),
            ("color(a98-rgb 0 0 0)", "rgb(0, 0, 0)"),
            ("color(prophoto-rgb 1 1 1)", "rgb(255, 255, 255)"),
            // Fuera de la gama de sRGB: gamut mapping en OKLCh (§13.2), que baja el croma
            // conservando la luminosidad (recortar los canales daría `rgb(0, 255, 0)`).
            ("color(display-p3 0 1 0)", "rgb(0, 251, 41)"),
            // Sintaxis legacy con comas: inválida (el color se hereda: negro).
            ("lab(50, 0, 0)", "rgb(0, 0, 0)"),
            ("color(nope 1 0 0)", "rgb(0, 0, 0)"),
        ],
        color,
    );
}

#[test]
fn color_mix() {
    check(
        &[
            // CSS Color 5 §2: ejemplos de la spec.
            ("color-mix(in srgb, red, blue)", "rgb(128, 0, 128)"),
            ("color-mix(in srgb, red 25%, blue)", "rgb(64, 0, 191)"),
            ("color-mix(in srgb, red, blue 75%)", "rgb(64, 0, 191)"),
            // Porcentajes que no suman 100: se normalizan; si suman menos, el alfa baja.
            (
                "color-mix(in srgb, red 20%, blue 60%)",
                "rgba(64, 0, 191, 0.8)",
            ),
            ("color-mix(in srgb, red 50%, blue 50%)", "rgb(128, 0, 128)"),
            // Alfa premultiplicado (§12.3).
            (
                "color-mix(in srgb, red, transparent)",
                "rgba(255, 0, 0, 0.5)",
            ),
            ("color-mix(in srgb-linear, red, blue)", "rgb(188, 0, 188)"),
            ("color-mix(in oklab, white, black)", "rgb(99, 99, 99)"),
            // Polares: el tono más corto y los demás métodos (§12.4).
            ("color-mix(in hsl, red, lime)", "rgb(255, 255, 0)"),
            ("color-mix(in hsl longer hue, red, lime)", "rgb(0, 0, 255)"),
            (
                "color-mix(in hsl increasing hue, red, lime)",
                "rgb(255, 255, 0)",
            ),
            (
                "color-mix(in hsl decreasing hue, red, lime)",
                "rgb(0, 0, 255)",
            ),
            // Un gris no tiene tono: se toma el del otro color (hsl(240 50% 75%)).
            ("color-mix(in hsl, white, blue)", "rgb(159, 159, 223)"),
            ("color-mix(in lch, red, red)", "rgb(255, 0, 0)"),
            // Anidado.
            (
                "color-mix(in srgb, color-mix(in srgb, red, blue), blue)",
                "rgb(64, 0, 191)",
            ),
            // Inválidos: el color se hereda.
            ("color-mix(in srgb, red)", "rgb(0, 0, 0)"),
            ("color-mix(in srgb, red 0%, blue 0%)", "rgb(0, 0, 0)"),
            ("color-mix(in srgb, red 120%, blue)", "rgb(0, 0, 0)"),
            ("color-mix(in srgb longer hue, red, blue)", "rgb(0, 0, 0)"),
            ("color-mix(in nope, red, blue)", "rgb(0, 0, 0)"),
        ],
        color,
    );
    // Sin método: OKLab.
    assert_eq!(
        color("color-mix(white, black)"),
        color("color-mix(in oklab, white, black)")
    );
    // Fondo (otra propiedad de color).
    assert_eq!(
        computed(
            "<!DOCTYPE html><div style=\"background-color: color-mix(in srgb, white 50%, black)\">x</div>",
            "div",
            "background-color"
        ),
        "rgb(128, 128, 128)"
    );
}

#[test]
fn env_function() {
    let width = |value: &str| {
        computed(
            &format!("<!DOCTYPE html><div style=\"width: {value}\">x</div>"),
            "div",
            "width",
        )
    };
    check(
        &[
            ("env(safe-area-inset-top)", "0px"),
            ("env(safe-area-inset-left, 20px)", "0px"),
            ("calc(10px + env(safe-area-inset-bottom))", "10px"),
            ("env(keyboard-inset-height, 5px)", "0px"),
            ("env(safe-area-max-inset-right)", "0px"),
            // Variable desconocida: el fallback (que puede tener `var()`).
            ("env(nope, 30px)", "30px"),
            ("env(titlebar-area-width, 40px)", "40px"),
            // Sin fallback: inválida en computed-value time (`width` vuelve a `auto`).
            ("env(nope)", "auto"),
        ],
        width,
    );
    assert_eq!(
        computed(
            "<!DOCTYPE html><div style=\"--f: 7px; width: env(nope, var(--f))\">x</div>",
            "div",
            "width"
        ),
        "7px"
    );
    // En custom properties también.
    assert_eq!(
        computed(
            "<!DOCTYPE html><div style=\"--pad: env(safe-area-inset-top, 3px); padding-top: var(--pad)\">x</div>",
            "div",
            "padding-top"
        ),
        "0px"
    );
}

#[test]
fn property_rule() {
    let doc = |css: &str, body: &str| format!("<!DOCTYPE html><style>{css}</style>{body}");
    let reg = "@property --len { syntax: '<length>'; inherits: false; initial-value: 5px }
        @property --col { syntax: '<color>'; inherits: true; initial-value: blue }
        @property --any { syntax: '*'; inherits: false }";
    // Valor inicial sin declaración, y `var()` lo ve.
    assert_eq!(computed(&doc(reg, "<div>x</div>"), "div", "--len"), "5px");
    assert_eq!(
        computed(
            &doc(
                &format!("{reg} div {{ width: var(--len) }}"),
                "<div>x</div>"
            ),
            "div",
            "width"
        ),
        "5px"
    );
    // No hereda: el hijo vuelve al valor inicial.
    assert_eq!(
        computed(
            &doc(
                &format!("{reg} #p {{ --len: 20px }}"),
                "<div id=p><span>x</span></div>"
            ),
            "span",
            "--len"
        ),
        "5px"
    );
    // Hereda (y el computado es el color resuelto).
    assert_eq!(
        computed(
            &doc(
                &format!("{reg} #p {{ --col: hsl(120 100% 50%) }}"),
                "<div id=p><span>x</span></div>"
            ),
            "span",
            "--col"
        ),
        "rgb(0, 255, 0)"
    );
    // Valor computado: longitudes absolutas.
    assert_eq!(
        computed(
            &doc(
                &format!("{reg} div {{ font-size: 10px; --len: 3em }}"),
                "<div>x</div>"
            ),
            "div",
            "--len"
        ),
        "30px"
    );
    // No cumple la sintaxis: inválida en computed-value time → `unset` (= inicial).
    assert_eq!(
        computed(
            &doc(&format!("{reg} div {{ --len: red }}"), "<div>x</div>"),
            "div",
            "--len"
        ),
        "5px"
    );
    assert_eq!(
        computed(
            &doc(
                &format!("{reg} div {{ --x: red; --len: var(--x) }}"),
                "<div>x</div>"
            ),
            "div",
            "--len"
        ),
        "5px"
    );
    // `initial` vuelve al valor inicial registrado.
    assert_eq!(
        computed(
            &doc(&format!("{reg} div {{ --len: initial }}"), "<div>x</div>"),
            "div",
            "--len"
        ),
        "5px"
    );
    // Sintaxis universal sin valor inicial: guaranteed-invalid (el fallback de `var()`).
    assert_eq!(
        computed(
            &doc(
                &format!("{reg} div {{ width: var(--any, 9px) }}"),
                "<div>x</div>"
            ),
            "div",
            "width"
        ),
        "9px"
    );
    // Reglas inválidas (sin `inherits`, valor inicial dependiente o que no cumple) se
    // ignoran: la propiedad queda sin registrar.
    for bad in [
        "@property --b { syntax: '<length>'; initial-value: 1px }",
        "@property --b { syntax: '<length>'; inherits: true; initial-value: 1em }",
        "@property --b { syntax: '<length>'; inherits: true; initial-value: red }",
        "@property --b { syntax: '<length>'; inherits: true }",
        "@property b { syntax: '*'; inherits: true }",
    ] {
        assert_eq!(
            computed(
                &doc(
                    &format!("{bad} div {{ width: var(--b, 8px) }}"),
                    "<div>x</div>"
                ),
                "div",
                "width"
            ),
            "8px",
            "{bad}"
        );
    }
    // Dentro de `@media` y `@supports`; la última registración gana.
    assert_eq!(
        computed(
            &doc(
                "@media all { @property --m { syntax: '<number>'; inherits: false; initial-value: 1 } }
                 @supports (display: block) { @property --m { syntax: '<number>'; inherits: false; initial-value: 2 } }",
                "<div>x</div>"
            ),
            "div",
            "--m"
        ),
        "2"
    );
}

#[test]
fn math_functions() {
    let width = |value: &str| {
        computed(
            &format!("<!DOCTYPE html><div style=\"width: {value}\">x</div>"),
            "div",
            "width",
        )
    };
    check(
        &[
            // §10.5: round(), mod(), rem().
            ("round(17px, 5px)", "15px"),
            ("round(up, 17px, 5px)", "20px"),
            ("round(down, 17px, 5px)", "15px"),
            ("round(to-zero, -17px, 5px)", "-15px"),
            ("round(nearest, 2.5px, 1px)", "3px"),
            ("calc(round(-2.5px, 1px) + 10px)", "8px"),
            ("mod(18px, 5px)", "3px"),
            ("calc(mod(-18px, 5px) + 0px)", "2px"),
            ("calc(rem(-18px, 5px) + 10px)", "7px"),
            // §10.6–§10.8: trigonométricas, exponenciales y signo (número × longitud).
            ("calc(100px * sin(30deg))", "50px"),
            ("calc(100px * cos(0))", "100px"),
            ("calc(100px * tan(45deg))", "100px"),
            ("calc(10px * pow(2, 3))", "80px"),
            ("calc(10px * sqrt(16))", "40px"),
            ("hypot(30px, 40px)", "50px"),
            ("calc(10px * log(100, 10))", "20px"),
            ("calc(10px * exp(0))", "10px"),
            ("abs(-12px)", "12px"),
            ("calc(10px * sign(-3px))", "-10px"),
            // División entre valores del mismo tipo: un número.
            ("calc(atan2(1px, 1px) / 1deg * 1px)", "45px"),
            ("calc(asin(1) / 1deg * 1px)", "90px"),
            ("calc(200px / 10px * 3px)", "60px"),
            ("calc(10px / 1deg)", "auto"),
            // `clamp()` con `none`.
            ("clamp(none, 50px, 20px)", "20px"),
            ("clamp(30px, 10px, none)", "30px"),
            // Constantes: infinito se recorta, NaN es 0.
            ("calc(infinity * 1px)", "33554428px"),
            ("calc(NaN * 1px)", "0px"),
            ("calc(e * 0px)", "0px"),
            // Inválidas (tipos): `width` queda en `auto`.
            ("sin(10px)", "auto"),
            ("round(10px)", "auto"),
            ("mod(10px, 2)", "auto"),
            ("pow(2px, 2)", "auto"),
            ("calc(10px + 1deg)", "auto"),
        ],
        width,
    );
}

#[test]
fn viewport_units() {
    // MediaContext::default(): viewport de 1280 × 720.
    let width = |value: &str| {
        computed(
            &format!("<!DOCTYPE html><div style=\"width: {value}\">x</div>"),
            "div",
            "width",
        )
    };
    check(
        &[
            ("10dvh", "72px"),
            ("10svh", "72px"),
            ("10lvh", "72px"),
            ("10dvw", "128px"),
            ("10svmin", "72px"),
            ("10lvmax", "128px"),
            ("10dvb", "72px"),
            ("10vi", "128px"),
            ("10vb", "72px"),
            // Sin contenedor de consulta: como las small viewport units.
            ("10cqw", "128px"),
            ("10cqh", "72px"),
            ("10cqmin", "72px"),
            ("calc(100dvh - 10px)", "710px"),
        ],
        width,
    );
}
