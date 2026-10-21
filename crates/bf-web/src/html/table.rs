//! Las interfaces de los elementos HTML (HTML §3.2.8, §4 y §15.3) y los atributos IDL que
//! reflejan atributos de contenido (las anotaciones `[Reflect]` del IDL), por interfaz.

use super::reflect::{EnumSpec, Reflect};

/// Cómo se implementa un atributo IDL de la tabla.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// Reflexión según §2.6.1.
    R(Reflect),
    /// `[LegacyNullToEmptyString] attribute DOMString`: `null` se convierte en `""`.
    StrNullEmpty,
    /// `[SameObject, PutForwards=value] readonly attribute DOMTokenList`.
    TokenList,
}

/// Un atributo IDL reflejado.
#[derive(Debug, Clone, Copy)]
pub struct Attr {
    pub idl: &'static str,
    pub content: &'static str,
    pub kind: Kind,
}

const fn a(idl: &'static str, content: &'static str, kind: Kind) -> Attr {
    Attr { idl, content, kind }
}
const fn s(name: &'static str) -> Attr {
    a(name, name, Kind::R(Reflect::Str))
}
const fn s_as(idl: &'static str, content: &'static str) -> Attr {
    a(idl, content, Kind::R(Reflect::Str))
}
const fn ne(idl: &'static str, content: &'static str) -> Attr {
    a(idl, content, Kind::StrNullEmpty)
}
const fn url(name: &'static str) -> Attr {
    a(name, name, Kind::R(Reflect::Url))
}
const fn url_as(idl: &'static str, content: &'static str) -> Attr {
    a(idl, content, Kind::R(Reflect::Url))
}
const fn b(name: &'static str) -> Attr {
    a(name, name, Kind::R(Reflect::Bool))
}
const fn b_as(idl: &'static str, content: &'static str) -> Attr {
    a(idl, content, Kind::R(Reflect::Bool))
}
const fn e(idl: &'static str, content: &'static str, spec: &'static EnumSpec) -> Attr {
    a(idl, content, Kind::R(Reflect::Enum(spec)))
}
const fn tl(idl: &'static str, content: &'static str) -> Attr {
    a(idl, content, Kind::TokenList)
}
const fn ul(name: &'static str, default: u32) -> Attr {
    a(name, name, Kind::R(Reflect::ULong(default)))
}
const fn long(name: &'static str, default: i32) -> Attr {
    a(name, name, Kind::R(Reflect::Long(default)))
}
const fn llong(idl: &'static str, content: &'static str) -> Attr {
    a(idl, content, Kind::R(Reflect::LimitedLong(-1)))
}
const fn clamped(idl: &'static str, default: u32, min: u32, max: u32) -> Attr {
    a(
        idl,
        idl,
        Kind::R(Reflect::ClampedULong { default, min, max }),
    )
}

const fn spec(
    keywords: &'static [&'static str],
    missing: Option<&'static str>,
    invalid: Option<&'static str>,
) -> EnumSpec {
    EnumSpec {
        keywords,
        aliases: &[],
        missing,
        invalid,
        empty: None,
        nullable: false,
    }
}

// --- Atributos enumerados (§2.3.3) ---

/// CORS settings attributes (§2.5.4): `DOMString?`; el valor vacío es `anonymous`.
pub static CORS: EnumSpec = EnumSpec {
    keywords: &["anonymous", "use-credentials"],
    aliases: &[("", "anonymous")],
    missing: None,
    invalid: Some("anonymous"),
    empty: None,
    nullable: true,
};
/// Referrer policy attributes (§2.5.5).
pub static REFERRER_POLICY: EnumSpec = spec(
    &[
        "",
        "no-referrer",
        "no-referrer-when-downgrade",
        "same-origin",
        "origin",
        "strict-origin",
        "origin-when-cross-origin",
        "strict-origin-when-cross-origin",
        "unsafe-url",
    ],
    None,
    None,
);
pub static DIR: EnumSpec = spec(&["ltr", "rtl", "auto"], None, None);
const ENCTYPES: &[&str] = &[
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "text/plain",
];
pub static FORM_AUTOCOMPLETE: EnumSpec = spec(&["on", "off"], Some("on"), Some("on"));
pub static ENCTYPE: EnumSpec = spec(
    ENCTYPES,
    Some("application/x-www-form-urlencoded"),
    Some("application/x-www-form-urlencoded"),
);
pub static FORM_METHOD: EnumSpec = spec(&["get", "post", "dialog"], Some("get"), Some("get"));
pub static FORM_ENCTYPE: EnumSpec = spec(ENCTYPES, None, Some("application/x-www-form-urlencoded"));
pub static INPUT_FORM_METHOD: EnumSpec = spec(&["get", "post"], None, Some("get"));
pub static BUTTON_FORM_METHOD: EnumSpec = spec(&["get", "post", "dialog"], None, Some("get"));
pub static INPUT_TYPE: EnumSpec = spec(
    &[
        "hidden",
        "text",
        "search",
        "tel",
        "url",
        "email",
        "password",
        "date",
        "month",
        "week",
        "time",
        "datetime-local",
        "number",
        "range",
        "color",
        "checkbox",
        "radio",
        "file",
        "submit",
        "image",
        "reset",
        "button",
    ],
    Some("text"),
    Some("text"),
);
pub static BUTTON_TYPE: EnumSpec = spec(
    &["submit", "reset", "button"],
    Some("submit"),
    Some("submit"),
);
pub static LINK_AS: EnumSpec = spec(
    &[
        "fetch",
        "audio",
        "document",
        "embed",
        "font",
        "image",
        "manifest",
        "object",
        "report",
        "script",
        "sharedworker",
        "style",
        "track",
        "video",
        "worker",
        "xslt",
    ],
    None,
    None,
);
pub static SCOPE: EnumSpec = spec(&["row", "col", "rowgroup", "colgroup"], None, None);
pub static DECODING: EnumSpec = spec(&["async", "sync", "auto"], Some("auto"), Some("auto"));
/// `preload` (§4.8.11.5): los defaults son del UA; se usa el sugerido, Metadata.
pub static PRELOAD: EnumSpec = EnumSpec {
    keywords: &["none", "metadata", "auto"],
    aliases: &[("", "auto")],
    missing: Some("metadata"),
    invalid: Some("metadata"),
    empty: None,
    nullable: false,
};
pub static LOADING: EnumSpec = spec(&["lazy", "eager"], Some("eager"), Some("eager"));
pub static TRACK_KIND: EnumSpec = spec(
    &[
        "subtitles",
        "captions",
        "descriptions",
        "chapters",
        "metadata",
    ],
    Some("subtitles"),
    Some("metadata"),
);
pub static MARQUEE_BEHAVIOR: EnumSpec = spec(
    &["scroll", "slide", "alternate"],
    Some("scroll"),
    Some("scroll"),
);
pub static MARQUEE_DIRECTION: EnumSpec =
    spec(&["up", "right", "down", "left"], Some("left"), Some("left"));
pub static ENTER_KEY_HINT: EnumSpec = spec(
    &["enter", "done", "go", "next", "previous", "search", "send"],
    None,
    None,
);
pub static INPUT_MODE: EnumSpec = spec(
    &[
        "none", "text", "tel", "url", "email", "numeric", "decimal", "search",
    ],
    None,
    None,
);

/// Una interfaz de elemento HTML: su nombre, la interfaz padre y sus atributos reflejados.
#[derive(Debug)]
pub struct Interface {
    pub name: &'static str,
    pub parent: &'static str,
    pub attrs: &'static [Attr],
}

const fn iface(name: &'static str, attrs: &'static [Attr]) -> Interface {
    Interface {
        name,
        parent: "HTMLElement",
        attrs,
    }
}

/// `HTMLElement` (§3.2.8) y los mixins de HTMLOrSVGElement / ElementContentEditable que
/// reflejan atributos.
pub static HTML_ELEMENT_ATTRS: &[Attr] = &[
    s("title"),
    s("lang"),
    e("dir", "dir", &DIR),
    b("hidden"),
    b("inert"),
    s_as("accessKey", "accesskey"),
    b("autofocus"),
    s("nonce"),
    e("enterKeyHint", "enterkeyhint", &ENTER_KEY_HINT),
    e("inputMode", "inputmode", &INPUT_MODE),
];

const TABLE_CELL_COMMON: &[Attr] = &[
    clamped("colSpan", 1, 1, 1000),
    clamped("rowSpan", 1, 0, 65534),
    s("headers"),
    e("scope", "scope", &SCOPE),
    s("abbr"),
    s("align"),
    s("axis"),
    s("height"),
    s("width"),
    s_as("ch", "char"),
    s_as("chOff", "charoff"),
    b_as("noWrap", "nowrap"),
    s_as("vAlign", "valign"),
    ne("bgColor", "bgcolor"),
];

const TABLE_SECTION_COMMON: &[Attr] = &[
    s("align"),
    s_as("ch", "char"),
    s_as("chOff", "charoff"),
    s_as("vAlign", "valign"),
];

const TABLE_COL_COMMON: &[Attr] = &[
    clamped("span", 1, 1, 1000),
    s("align"),
    s_as("ch", "char"),
    s_as("chOff", "charoff"),
    s_as("vAlign", "valign"),
    s("width"),
];

const MEDIA_COMMON: &[Attr] = &[
    url("src"),
    e("crossOrigin", "crossorigin", &CORS),
    e("preload", "preload", &PRELOAD),
    b("autoplay"),
    b("loop"),
    b("controls"),
    b_as("defaultMuted", "muted"),
];

/// Las interfaces de elemento con sus atributos propios (sin los de HTMLElement).
pub static INTERFACES: &[Interface] = &[
    iface(
        "HTMLAnchorElement",
        &[
            s("target"),
            s("download"),
            s("ping"),
            s("rel"),
            tl("relList", "rel"),
            s("hreflang"),
            s("type"),
            e("referrerPolicy", "referrerpolicy", &REFERRER_POLICY),
            s("coords"),
            s("charset"),
            s("name"),
            s("rev"),
            s("shape"),
        ],
    ),
    iface(
        "HTMLAreaElement",
        &[
            s("alt"),
            s("coords"),
            s("shape"),
            s("target"),
            s("download"),
            s("ping"),
            s("rel"),
            tl("relList", "rel"),
            e("referrerPolicy", "referrerpolicy", &REFERRER_POLICY),
            b_as("noHref", "nohref"),
        ],
    ),
    Interface {
        name: "HTMLMediaElement",
        parent: "HTMLElement",
        attrs: MEDIA_COMMON,
    },
    Interface {
        name: "HTMLAudioElement",
        parent: "HTMLMediaElement",
        attrs: &[],
    },
    Interface {
        name: "HTMLVideoElement",
        parent: "HTMLMediaElement",
        attrs: &[
            ul("width", 0),
            ul("height", 0),
            url("poster"),
            b_as("playsInline", "playsinline"),
        ],
    },
    iface("HTMLBaseElement", &[s("target")]),
    iface(
        "HTMLBodyElement",
        &[
            ne("text", "text"),
            ne("link", "link"),
            ne("vLink", "vlink"),
            ne("aLink", "alink"),
            ne("bgColor", "bgcolor"),
            s("background"),
        ],
    ),
    iface("HTMLBRElement", &[s("clear")]),
    iface(
        "HTMLButtonElement",
        &[
            b("disabled"),
            url_as("formAction", "formaction"),
            e("formEnctype", "formenctype", &FORM_ENCTYPE),
            e("formMethod", "formmethod", &BUTTON_FORM_METHOD),
            b_as("formNoValidate", "formnovalidate"),
            s_as("formTarget", "formtarget"),
            s("name"),
            e("type", "type", &BUTTON_TYPE),
            s("value"),
        ],
    ),
    iface("HTMLCanvasElement", &[ul("width", 300), ul("height", 150)]),
    iface("HTMLDataElement", &[s("value")]),
    iface("HTMLDataListElement", &[]),
    iface("HTMLDetailsElement", &[b("open")]),
    iface("HTMLDialogElement", &[b("open")]),
    iface("HTMLDivElement", &[s("align")]),
    iface("HTMLDListElement", &[b("compact")]),
    iface(
        "HTMLEmbedElement",
        &[
            url("src"),
            s("type"),
            s("width"),
            s("height"),
            s("align"),
            s("name"),
        ],
    ),
    iface("HTMLFieldSetElement", &[b("disabled"), s("name")]),
    iface(
        "HTMLFormElement",
        &[
            s_as("acceptCharset", "accept-charset"),
            e("autocomplete", "autocomplete", &FORM_AUTOCOMPLETE),
            e("enctype", "enctype", &ENCTYPE),
            e("encoding", "enctype", &ENCTYPE),
            e("method", "method", &FORM_METHOD),
            s("name"),
            b_as("noValidate", "novalidate"),
            s("target"),
            s("rel"),
            tl("relList", "rel"),
        ],
    ),
    iface("HTMLHeadingElement", &[s("align")]),
    iface("HTMLHeadElement", &[]),
    iface(
        "HTMLHRElement",
        &[
            s("align"),
            s("color"),
            b_as("noShade", "noshade"),
            s("size"),
            s("width"),
        ],
    ),
    iface("HTMLHtmlElement", &[s("version")]),
    iface(
        "HTMLIFrameElement",
        &[
            url("src"),
            s("srcdoc"),
            s("name"),
            tl("sandbox", "sandbox"),
            b_as("allowFullscreen", "allowfullscreen"),
            s("width"),
            s("height"),
            e("referrerPolicy", "referrerpolicy", &REFERRER_POLICY),
            e("loading", "loading", &LOADING),
            s("align"),
            s("scrolling"),
            s_as("frameBorder", "frameborder"),
            url_as("longDesc", "longdesc"),
            ne("marginHeight", "marginheight"),
            ne("marginWidth", "marginwidth"),
        ],
    ),
    iface(
        "HTMLImageElement",
        &[
            s("alt"),
            url("src"),
            s("srcset"),
            s("sizes"),
            e("crossOrigin", "crossorigin", &CORS),
            s_as("useMap", "usemap"),
            b_as("isMap", "ismap"),
            e("referrerPolicy", "referrerpolicy", &REFERRER_POLICY),
            e("decoding", "decoding", &DECODING),
            e("loading", "loading", &LOADING),
            s("name"),
            url("lowsrc"),
            s("align"),
            ul("hspace", 0),
            ul("vspace", 0),
            url_as("longDesc", "longdesc"),
            ne("border", "border"),
        ],
    ),
    iface(
        "HTMLInputElement",
        &[
            s("accept"),
            s("alt"),
            b_as("defaultChecked", "checked"),
            s_as("dirName", "dirname"),
            b("disabled"),
            url_as("formAction", "formaction"),
            e("formEnctype", "formenctype", &FORM_ENCTYPE),
            e("formMethod", "formmethod", &INPUT_FORM_METHOD),
            b_as("formNoValidate", "formnovalidate"),
            s_as("formTarget", "formtarget"),
            s("max"),
            llong("maxLength", "maxlength"),
            s("min"),
            llong("minLength", "minlength"),
            b("multiple"),
            s("name"),
            s("pattern"),
            s("placeholder"),
            b_as("readOnly", "readonly"),
            b("required"),
            a(
                "size",
                "size",
                Kind::R(Reflect::LimitedULong {
                    default: 20,
                    fallback: false,
                }),
            ),
            url("src"),
            s("step"),
            e("type", "type", &INPUT_TYPE),
            s_as("defaultValue", "value"),
            s("align"),
            s_as("useMap", "usemap"),
        ],
    ),
    iface("HTMLLabelElement", &[s_as("htmlFor", "for")]),
    iface("HTMLLegendElement", &[s("align")]),
    iface("HTMLLIElement", &[long("value", 0), s("type")]),
    iface(
        "HTMLLinkElement",
        &[
            url("href"),
            e("crossOrigin", "crossorigin", &CORS),
            s("rel"),
            e("as", "as", &LINK_AS),
            tl("relList", "rel"),
            s("media"),
            s("integrity"),
            s("hreflang"),
            s("type"),
            tl("sizes", "sizes"),
            e("referrerPolicy", "referrerpolicy", &REFERRER_POLICY),
            b("disabled"),
            s("charset"),
            s("rev"),
            s("target"),
        ],
    ),
    iface("HTMLMapElement", &[s("name")]),
    iface(
        "HTMLMarqueeElement",
        &[
            e("behavior", "behavior", &MARQUEE_BEHAVIOR),
            s_as("bgColor", "bgcolor"),
            e("direction", "direction", &MARQUEE_DIRECTION),
            s("height"),
            ul("hspace", 0),
            ul("scrollAmount", 6),
            ul("scrollDelay", 85),
            b_as("trueSpeed", "truespeed"),
            ul("vspace", 0),
            s("width"),
        ],
    ),
    iface("HTMLMenuElement", &[b("compact")]),
    iface(
        "HTMLMetaElement",
        &[
            s("name"),
            s_as("httpEquiv", "http-equiv"),
            s("content"),
            s("media"),
            s("scheme"),
        ],
    ),
    iface("HTMLMeterElement", &[]),
    iface(
        "HTMLModElement",
        &[url("cite"), s_as("dateTime", "datetime")],
    ),
    iface(
        "HTMLObjectElement",
        &[
            url("data"),
            s("type"),
            s("name"),
            s_as("useMap", "usemap"),
            s("width"),
            s("height"),
            s("align"),
            s("archive"),
            s("code"),
            b("declare"),
            ul("hspace", 0),
            s("standby"),
            ul("vspace", 0),
            url_as("codeBase", "codebase"),
            s_as("codeType", "codetype"),
            ne("border", "border"),
        ],
    ),
    iface(
        "HTMLOListElement",
        &[b("reversed"), long("start", 1), s("type"), b("compact")],
    ),
    iface("HTMLOptGroupElement", &[b("disabled"), s("label")]),
    iface(
        "HTMLOptionElement",
        &[b("disabled"), b_as("defaultSelected", "selected")],
    ),
    iface("HTMLOutputElement", &[tl("htmlFor", "for"), s("name")]),
    iface("HTMLParagraphElement", &[s("align")]),
    iface(
        "HTMLParamElement",
        &[
            s("name"),
            s("value"),
            s("type"),
            s_as("valueType", "valuetype"),
        ],
    ),
    iface("HTMLPictureElement", &[]),
    iface("HTMLPreElement", &[long("width", 0)]),
    iface(
        "HTMLProgressElement",
        &[a("max", "max", Kind::R(Reflect::LimitedDouble(1.0)))],
    ),
    iface("HTMLQuoteElement", &[url("cite")]),
    iface(
        "HTMLScriptElement",
        &[
            url("src"),
            s("type"),
            b_as("noModule", "nomodule"),
            b("async"),
            b("defer"),
            e("crossOrigin", "crossorigin", &CORS),
            s("integrity"),
            e("referrerPolicy", "referrerpolicy", &REFERRER_POLICY),
            s("charset"),
            s("event"),
            s_as("htmlFor", "for"),
        ],
    ),
    iface(
        "HTMLSelectElement",
        &[
            b("disabled"),
            b("multiple"),
            s("name"),
            b("required"),
            ul("size", 0),
        ],
    ),
    iface("HTMLSlotElement", &[s("name")]),
    iface(
        "HTMLSourceElement",
        &[
            url("src"),
            s("type"),
            s("srcset"),
            s("sizes"),
            s("media"),
            ul("width", 0),
            ul("height", 0),
        ],
    ),
    iface("HTMLSpanElement", &[]),
    iface("HTMLStyleElement", &[s("media"), s("type")]),
    iface("HTMLTableCaptionElement", &[s("align")]),
    Interface {
        name: "HTMLTableCellElement",
        parent: "HTMLElement",
        attrs: TABLE_CELL_COMMON,
    },
    Interface {
        name: "HTMLTableColElement",
        parent: "HTMLElement",
        attrs: TABLE_COL_COMMON,
    },
    iface(
        "HTMLTableElement",
        &[
            s("align"),
            s("border"),
            s("frame"),
            s("rules"),
            s("summary"),
            s("width"),
            ne("bgColor", "bgcolor"),
            ne("cellPadding", "cellpadding"),
            ne("cellSpacing", "cellspacing"),
        ],
    ),
    iface(
        "HTMLTableRowElement",
        &[
            s("align"),
            s_as("ch", "char"),
            s_as("chOff", "charoff"),
            s_as("vAlign", "valign"),
            ne("bgColor", "bgcolor"),
        ],
    ),
    Interface {
        name: "HTMLTableSectionElement",
        parent: "HTMLElement",
        attrs: TABLE_SECTION_COMMON,
    },
    iface("HTMLTemplateElement", &[]),
    iface(
        "HTMLTextAreaElement",
        &[
            s_as("dirName", "dirname"),
            b("disabled"),
            llong("maxLength", "maxlength"),
            llong("minLength", "minlength"),
            s("name"),
            s("placeholder"),
            b_as("readOnly", "readonly"),
            b("required"),
            a(
                "cols",
                "cols",
                Kind::R(Reflect::LimitedULong {
                    default: 20,
                    fallback: true,
                }),
            ),
            a(
                "rows",
                "rows",
                Kind::R(Reflect::LimitedULong {
                    default: 2,
                    fallback: true,
                }),
            ),
            s("wrap"),
        ],
    ),
    iface("HTMLTimeElement", &[s_as("dateTime", "datetime")]),
    iface("HTMLTitleElement", &[]),
    iface(
        "HTMLTrackElement",
        &[
            e("kind", "kind", &TRACK_KIND),
            url("src"),
            s("srclang"),
            s("label"),
            b("default"),
        ],
    ),
    iface("HTMLUListElement", &[b("compact"), s("type")]),
    iface("HTMLUnknownElement", &[]),
    iface("HTMLDirectoryElement", &[b("compact")]),
    iface(
        "HTMLFontElement",
        &[ne("color", "color"), s("face"), s("size")],
    ),
    iface(
        "HTMLFrameElement",
        &[
            s("name"),
            s("scrolling"),
            url("src"),
            s_as("frameBorder", "frameborder"),
            url_as("longDesc", "longdesc"),
            b_as("noResize", "noresize"),
            ne("marginHeight", "marginheight"),
            ne("marginWidth", "marginwidth"),
        ],
    ),
    iface("HTMLFrameSetElement", &[s("cols"), s("rows")]),
];

/// La interfaz de un elemento HTML por su nombre local (HTML §15.3 y las secciones de cada
/// elemento; "element interface" de §3.2.8 para los demás).
pub fn interface_for(local: &str) -> &'static str {
    match local {
        "a" => "HTMLAnchorElement",
        "area" => "HTMLAreaElement",
        "audio" => "HTMLAudioElement",
        "base" => "HTMLBaseElement",
        "blockquote" | "q" => "HTMLQuoteElement",
        "body" => "HTMLBodyElement",
        "br" => "HTMLBRElement",
        "button" => "HTMLButtonElement",
        "canvas" => "HTMLCanvasElement",
        "caption" => "HTMLTableCaptionElement",
        "col" | "colgroup" => "HTMLTableColElement",
        "data" => "HTMLDataElement",
        "datalist" => "HTMLDataListElement",
        "del" | "ins" => "HTMLModElement",
        "details" => "HTMLDetailsElement",
        "dialog" => "HTMLDialogElement",
        "dir" => "HTMLDirectoryElement",
        "div" => "HTMLDivElement",
        "dl" => "HTMLDListElement",
        "embed" => "HTMLEmbedElement",
        "fieldset" => "HTMLFieldSetElement",
        "font" => "HTMLFontElement",
        "form" => "HTMLFormElement",
        "frame" => "HTMLFrameElement",
        "frameset" => "HTMLFrameSetElement",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement",
        "head" => "HTMLHeadElement",
        "hr" => "HTMLHRElement",
        "html" => "HTMLHtmlElement",
        "iframe" => "HTMLIFrameElement",
        "img" => "HTMLImageElement",
        "input" => "HTMLInputElement",
        "label" => "HTMLLabelElement",
        "legend" => "HTMLLegendElement",
        "li" => "HTMLLIElement",
        "link" => "HTMLLinkElement",
        "map" => "HTMLMapElement",
        "marquee" => "HTMLMarqueeElement",
        "menu" => "HTMLMenuElement",
        "meta" => "HTMLMetaElement",
        "meter" => "HTMLMeterElement",
        "object" => "HTMLObjectElement",
        "ol" => "HTMLOListElement",
        "optgroup" => "HTMLOptGroupElement",
        "option" => "HTMLOptionElement",
        "output" => "HTMLOutputElement",
        "p" => "HTMLParagraphElement",
        "param" => "HTMLParamElement",
        "picture" => "HTMLPictureElement",
        "pre" | "listing" | "xmp" => "HTMLPreElement",
        "progress" => "HTMLProgressElement",
        "script" => "HTMLScriptElement",
        "select" => "HTMLSelectElement",
        "slot" => "HTMLSlotElement",
        "source" => "HTMLSourceElement",
        "span" => "HTMLSpanElement",
        "style" => "HTMLStyleElement",
        "table" => "HTMLTableElement",
        "tbody" | "thead" | "tfoot" => "HTMLTableSectionElement",
        "td" | "th" => "HTMLTableCellElement",
        "template" => "HTMLTemplateElement",
        "textarea" => "HTMLTextAreaElement",
        "time" => "HTMLTimeElement",
        "title" => "HTMLTitleElement",
        "tr" => "HTMLTableRowElement",
        "track" => "HTMLTrackElement",
        "ul" => "HTMLUListElement",
        "video" => "HTMLVideoElement",
        // §15.3: obsoletos sin interfaz propia.
        "applet" | "bgsound" | "blink" | "isindex" | "keygen" | "multicol" | "nextid"
        | "spacer" => "HTMLUnknownElement",
        // Los elementos de §4 que usan HTMLElement, y los obsoletos de §15.3 que también.
        "abbr" | "address" | "article" | "aside" | "b" | "bdi" | "bdo" | "cite" | "code" | "dd"
        | "dfn" | "dt" | "em" | "figcaption" | "figure" | "footer" | "header" | "hgroup" | "i"
        | "kbd" | "main" | "mark" | "nav" | "noscript" | "rp" | "rt" | "ruby" | "s" | "samp"
        | "search" | "section" | "small" | "strong" | "sub" | "summary" | "sup" | "u" | "var"
        | "wbr" | "acronym" | "basefont" | "big" | "center" | "nobr" | "noembed" | "noframes"
        | "plaintext" | "rb" | "rtc" | "strike" | "tt" => "HTMLElement",
        // Un nombre de custom element válido (sin definición): HTMLElement.
        other if is_valid_custom_element_name(other) => "HTMLElement",
        _ => "HTMLUnknownElement",
    }
}

/// "Valid custom element name" (HTML §4.13.2), aproximado: empieza con [a-z], contiene un
/// guion, no tiene mayúsculas ASCII y no es uno de los nombres reservados.
pub fn is_valid_custom_element_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.contains('-')
        && !name.chars().any(|c| c.is_ascii_uppercase())
        && !RESERVED.contains(&name)
}
