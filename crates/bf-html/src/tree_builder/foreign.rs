//! 13.2.6.5 The rules for parsing tokens in foreign content, y los ajustes de nombres de
//! 13.2.6.1 (adjust MathML/SVG/foreign attributes).

use bf_infra::Namespace;

use super::{
    Flow, TagToken, Tok, TreeBuilder, is_ascii_whitespace, is_html_integration_point,
    is_mathml_text_integration_point,
};

/// Start tags que hacen salir del contenido foráneo.
const BREAKOUT: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "body",
    "br",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "hr",
    "i",
    "img",
    "li",
    "listing",
    "menu",
    "meta",
    "nobr",
    "ol",
    "p",
    "pre",
    "ruby",
    "s",
    "small",
    "span",
    "strong",
    "strike",
    "sub",
    "sup",
    "table",
    "tt",
    "u",
    "ul",
    "var",
];

impl TreeBuilder {
    pub(super) fn foreign_content(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char('\0') => {
                self.error();
                self.insert_char('\u{FFFD}');
            }
            Tok::Char(c) if is_ascii_whitespace(c) => self.insert_char(c),
            Tok::Char(c) => {
                self.insert_char(c);
                self.frameset_ok = false;
            }
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t)
                if BREAKOUT.contains(&t.name.as_str())
                    || (t.name == "font"
                        && ["color", "face", "size"]
                            .iter()
                            .any(|a| t.attr(a).is_some())) =>
            {
                return self.foreign_breakout(tok);
            }
            Tok::End(ref t) if t.name == "br" || t.name == "p" => {
                return self.foreign_breakout(tok);
            }
            Tok::Start(mut tag) => {
                let acn = self.adjusted_current_node().expect("adjusted current node");
                let ns = self.elem(acn).ns;
                if ns == Namespace::MathMl {
                    adjust_mathml_attributes(&mut tag);
                }
                if ns == Namespace::Svg {
                    if let Some(name) = svg_tag_name(&tag.name) {
                        tag.name = name.into();
                    }
                    adjust_svg_attributes(&mut tag);
                }
                adjust_foreign_attributes(&mut tag);
                self.insert_foreign(&tag, ns);
                if tag.self_closing {
                    // Para `script` SVG: acknowledge y actuar como el end tag "script" de abajo,
                    // que sin soporte de scripts SVG solo saca el elemento de la stack.
                    self.pop();
                }
            }
            Tok::End(ref t)
                if t.name == "script"
                    && self.elem(self.current()).ns == Namespace::Svg
                    && self.elem(self.current()).name == "script" =>
            {
                self.pop();
            }
            Tok::End(ref t) => {
                let mut i = self.open.len() - 1;
                if !self.elem(self.open[i]).name.eq_ignore_ascii_case(&t.name) {
                    self.error();
                }
                loop {
                    if i == 0 {
                        return Flow::Done;
                    }
                    let node = self.open[i];
                    if self.elem(node).name.to_ascii_lowercase() == t.name {
                        self.truncate_open(i);
                        return Flow::Done;
                    }
                    i -= 1;
                    if self.elem(self.open[i]).ns == Namespace::Html {
                        return self.process_in(self.mode, tok);
                    }
                }
            }
            Tok::Eof => unreachable!("el dispatcher manda EOF a las reglas de HTML content"),
        }
        Flow::Done
    }

    /// Pop hasta un MathML text integration point, un HTML integration point o un elemento
    /// HTML, y reprocesar con las reglas del insertion mode actual en HTML content.
    fn foreign_breakout(&mut self, tok: Tok) -> Flow {
        self.error();
        loop {
            let el = self.elem(self.current());
            if is_mathml_text_integration_point(el)
                || is_html_integration_point(el)
                || el.ns == Namespace::Html
            {
                break;
            }
            self.pop();
        }
        self.process_in(self.mode, tok)
    }
}

/// Adjust MathML attributes.
pub(super) fn adjust_mathml_attributes(tag: &mut TagToken) {
    for attr in &mut tag.attrs {
        if attr.ns.is_none() && attr.name == "definitionurl" {
            attr.name = "definitionURL".into();
        }
    }
}

/// Adjust SVG attributes.
pub(super) fn adjust_svg_attributes(tag: &mut TagToken) {
    for attr in &mut tag.attrs {
        if attr.ns.is_none()
            && let Some(name) = svg_attribute_name(&attr.name)
        {
            attr.name = name.into();
        }
    }
}

/// Adjust foreign attributes.
pub(super) fn adjust_foreign_attributes(tag: &mut TagToken) {
    for attr in &mut tag.attrs {
        if attr.ns.is_some() {
            continue;
        }
        let adjusted = match attr.name.as_str() {
            "xlink:actuate" => Some((Some("xlink"), "actuate", Namespace::XLink)),
            "xlink:arcrole" => Some((Some("xlink"), "arcrole", Namespace::XLink)),
            "xlink:href" => Some((Some("xlink"), "href", Namespace::XLink)),
            "xlink:role" => Some((Some("xlink"), "role", Namespace::XLink)),
            "xlink:show" => Some((Some("xlink"), "show", Namespace::XLink)),
            "xlink:title" => Some((Some("xlink"), "title", Namespace::XLink)),
            "xlink:type" => Some((Some("xlink"), "type", Namespace::XLink)),
            "xml:lang" => Some((Some("xml"), "lang", Namespace::Xml)),
            "xml:space" => Some((Some("xml"), "space", Namespace::Xml)),
            "xmlns" => Some((None, "xmlns", Namespace::XmlNs)),
            "xmlns:xlink" => Some((Some("xmlns"), "xlink", Namespace::XmlNs)),
            _ => None,
        };
        if let Some((prefix, local, ns)) = adjusted {
            attr.prefix = prefix.map(Into::into);
            attr.name = local.into();
            attr.ns = Some(ns);
        }
    }
}

/// Tabla de ajuste de nombres de elementos SVG (13.2.6.5, "any other start tag").
fn svg_tag_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "altglyph" => "altGlyph",
        "altglyphdef" => "altGlyphDef",
        "altglyphitem" => "altGlyphItem",
        "animatecolor" => "animateColor",
        "animatemotion" => "animateMotion",
        "animatetransform" => "animateTransform",
        "clippath" => "clipPath",
        "feblend" => "feBlend",
        "fecolormatrix" => "feColorMatrix",
        "fecomponenttransfer" => "feComponentTransfer",
        "fecomposite" => "feComposite",
        "feconvolvematrix" => "feConvolveMatrix",
        "fediffuselighting" => "feDiffuseLighting",
        "fedisplacementmap" => "feDisplacementMap",
        "fedistantlight" => "feDistantLight",
        "fedropshadow" => "feDropShadow",
        "feflood" => "feFlood",
        "fefunca" => "feFuncA",
        "fefuncb" => "feFuncB",
        "fefuncg" => "feFuncG",
        "fefuncr" => "feFuncR",
        "fegaussianblur" => "feGaussianBlur",
        "feimage" => "feImage",
        "femerge" => "feMerge",
        "femergenode" => "feMergeNode",
        "femorphology" => "feMorphology",
        "feoffset" => "feOffset",
        "fepointlight" => "fePointLight",
        "fespecularlighting" => "feSpecularLighting",
        "fespotlight" => "feSpotLight",
        "fetile" => "feTile",
        "feturbulence" => "feTurbulence",
        "foreignobject" => "foreignObject",
        "glyphref" => "glyphRef",
        "lineargradient" => "linearGradient",
        "radialgradient" => "radialGradient",
        "textpath" => "textPath",
        _ => return None,
    })
}

/// Tabla de "adjust SVG attributes" (13.2.6.1).
fn svg_attribute_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "attributename" => "attributeName",
        "attributetype" => "attributeType",
        "basefrequency" => "baseFrequency",
        "baseprofile" => "baseProfile",
        "calcmode" => "calcMode",
        "clippathunits" => "clipPathUnits",
        "diffuseconstant" => "diffuseConstant",
        "edgemode" => "edgeMode",
        "filterunits" => "filterUnits",
        "glyphref" => "glyphRef",
        "gradienttransform" => "gradientTransform",
        "gradientunits" => "gradientUnits",
        "kernelmatrix" => "kernelMatrix",
        "kernelunitlength" => "kernelUnitLength",
        "keypoints" => "keyPoints",
        "keysplines" => "keySplines",
        "keytimes" => "keyTimes",
        "lengthadjust" => "lengthAdjust",
        "limitingconeangle" => "limitingConeAngle",
        "markerheight" => "markerHeight",
        "markerunits" => "markerUnits",
        "markerwidth" => "markerWidth",
        "maskcontentunits" => "maskContentUnits",
        "maskunits" => "maskUnits",
        "numoctaves" => "numOctaves",
        "pathlength" => "pathLength",
        "patterncontentunits" => "patternContentUnits",
        "patterntransform" => "patternTransform",
        "patternunits" => "patternUnits",
        "pointsatx" => "pointsAtX",
        "pointsaty" => "pointsAtY",
        "pointsatz" => "pointsAtZ",
        "preservealpha" => "preserveAlpha",
        "preserveaspectratio" => "preserveAspectRatio",
        "primitiveunits" => "primitiveUnits",
        "refx" => "refX",
        "refy" => "refY",
        "repeatcount" => "repeatCount",
        "repeatdur" => "repeatDur",
        "requiredextensions" => "requiredExtensions",
        "requiredfeatures" => "requiredFeatures",
        "specularconstant" => "specularConstant",
        "specularexponent" => "specularExponent",
        "spreadmethod" => "spreadMethod",
        "startoffset" => "startOffset",
        "stddeviation" => "stdDeviation",
        "stitchtiles" => "stitchTiles",
        "surfacescale" => "surfaceScale",
        "systemlanguage" => "systemLanguage",
        "tablevalues" => "tableValues",
        "targetx" => "targetX",
        "targety" => "targetY",
        "textlength" => "textLength",
        "viewbox" => "viewBox",
        "viewtarget" => "viewTarget",
        "xchannelselector" => "xChannelSelector",
        "ychannelselector" => "yChannelSelector",
        "zoomandpan" => "zoomAndPan",
        _ => return None,
    })
}
