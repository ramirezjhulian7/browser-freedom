//! Elementos `script` y el estado del documento para procesarlos (HTML §4.12.1): los slots
//! internos de cada `script`, la clasificación de "prepare the script element" (§4.12.1.1, pasos
//! 1 a 30), las listas de scripts del documento, la *current document readiness* (§3.1.3) y los
//! contadores de `document.write()` (§8.4).
//!
//! La búsqueda del recurso, la ejecución y las listas de espera las maneja el parser HTML
//! (`bf-html`), que es quien tiene el motor de script del otro lado.

use std::collections::HashMap;

use bf_infra::encoding::Encoding;
use bf_infra::{DomString, Url};

use crate::events::{EventInit, EventKind, EventTarget, ScriptHost};
use crate::{Document, NodeId};

/// El *type* de un `script` (§4.12.1: "classic", "module", "importmap").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptType {
    Classic,
    Module,
    ImportMap,
}

/// El *result* de un `script`: "uninitialized", null (falló) o un script listo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptResult {
    #[default]
    Uninitialized,
    Null,
    Ready,
}

/// Estado de un CORS settings attribute (§2.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsSetting {
    /// Sin atributo.
    NoCors,
    Anonymous,
    UseCredentials,
}

/// Estado del atributo `fetchpriority` (§2.5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchPriority {
    High,
    Low,
    Auto,
}

/// Qué hacer cuando el resultado está listo ("steps to run when the result is ready", paso 33).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnReady {
    /// En el *set of scripts that will execute as soon as possible*: ejecutarlo y sacarlo.
    AsSoonAsPossible,
    /// En la *list of scripts that will execute in order as soon as possible*.
    InOrder,
    /// Defer o parsing-blocking: marcarlo *ready to be parser-executed*; lo ejecuta el parser.
    ParserExecute,
}

/// La clasificación de "prepare the script element" (pasos 5 a 30): lo que necesita quien
/// busca y ejecuta el script.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptInfo {
    pub type_: ScriptType,
    /// El *script block's type string* (paso 8).
    pub type_string: DomString,
    /// *Source text*: el child text content al preparar (paso 5).
    pub source_text: DomString,
    /// Valor del atributo `src`, si lo tiene.
    pub src: Option<DomString>,
    /// `src` resuelto contra el documento (paso 31.5); `None` sin `src` o si no es una URL.
    pub url: Option<Url>,
    pub is_async: bool,
    pub defer: bool,
    pub nomodule: bool,
    /// *Parser metadata* (paso 28).
    pub parser_inserted: bool,
    /// Encoding del atributo `charset` o, si falta o no es válido, el del documento (paso 21).
    pub encoding: &'static str,
    /// *Classic script CORS setting* (paso 22).
    pub crossorigin: CorsSetting,
    /// *Integrity metadata* (paso 25).
    pub integrity: DomString,
    /// *Cryptographic nonce* (paso 24).
    pub nonce: DomString,
    /// *Referrer policy*: la palabra clave válida en minúsculas, o vacío (paso 26).
    pub referrer_policy: &'static str,
    pub fetch_priority: FetchPriority,
    /// La *document base URL* para scripts inline (paso 32.1).
    pub base_url: Url,
}

/// Los slots internos de un `script` (§4.12.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptElementState {
    /// *Parser document* no nulo: lo insertó el parser.
    pub parser_inserted: bool,
    /// *Force async* (inicialmente `true`).
    pub force_async: bool,
    pub already_started: bool,
    pub from_external_file: bool,
    pub ready_to_be_parser_executed: bool,
    pub delaying_load_event: bool,
    pub result: ScriptResult,
    pub on_ready: Option<OnReady>,
    /// La clasificación del último "prepare" que llegó al paso 30.
    pub info: Option<ScriptInfo>,
}

impl Default for ScriptElementState {
    fn default() -> Self {
        ScriptElementState {
            parser_inserted: false,
            force_async: true,
            already_started: false,
            from_external_file: false,
            ready_to_be_parser_executed: false,
            delaying_load_event: false,
            result: ScriptResult::Uninitialized,
            on_ready: None,
            info: None,
        }
    }
}

/// *Current document readiness* (§3.1.3, `document.readyState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DocumentReadiness {
    Loading,
    Interactive,
    #[default]
    Complete,
}

impl DocumentReadiness {
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentReadiness::Loading => "loading",
            DocumentReadiness::Interactive => "interactive",
            DocumentReadiness::Complete => "complete",
        }
    }
}

/// Estado de scripts del documento.
#[derive(Debug, Clone, Default)]
pub struct ScriptLists {
    states: HashMap<NodeId, ScriptElementState>,
    /// *List of scripts that will execute when the document has finished parsing* (defer).
    pub after_parsing: Vec<NodeId>,
    /// *Set of scripts that will execute as soon as possible* (async).
    pub as_soon_as_possible: Vec<NodeId>,
    /// *List of scripts that will execute in order as soon as possible*.
    pub in_order: Vec<NodeId>,
    /// *Pending parsing-blocking script*.
    pub pending_parsing_blocking: Option<NodeId>,
    pub readiness: DocumentReadiness,
    /// `document.currentScript`.
    pub current_script: Option<NodeId>,
    /// *Ignore-destructive-writes counter* (§8.4.3).
    pub ignore_destructive_writes: u32,
    /// *Throw-on-dynamic-markup-insertion counter* (§8.4.1).
    pub throw_on_dynamic_markup_insertion: u32,
}

/// "JavaScript MIME type essence match" (MIME Sniffing §4.6).
const JAVASCRIPT_MIME_TYPES: [&str; 16] = [
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

/// Palabras clave de `referrerpolicy` (Referrer Policy §3).
const REFERRER_POLICIES: [&str; 8] = [
    "no-referrer",
    "no-referrer-when-downgrade",
    "same-origin",
    "origin",
    "strict-origin",
    "origin-when-cross-origin",
    "strict-origin-when-cross-origin",
    "unsafe-url",
];

fn trim_ascii_whitespace(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' '))
}

impl Document {
    /// Estado de scripts del documento.
    pub fn scripts(&self) -> &ScriptLists {
        &self.scripts
    }

    pub fn scripts_mut(&mut self) -> &mut ScriptLists {
        &mut self.scripts
    }

    /// Los slots internos de un `script` (los valores iniciales si nunca se tocaron).
    pub fn script_state(&self, el: NodeId) -> ScriptElementState {
        self.scripts.states.get(&el).cloned().unwrap_or_default()
    }

    pub fn script_state_mut(&mut self, el: NodeId) -> &mut ScriptElementState {
        self.scripts.states.entry(el).or_default()
    }

    /// La clasificación del último "prepare the script element" de `el`.
    pub fn script_info(&self, el: NodeId) -> Option<&ScriptInfo> {
        self.scripts.states.get(&el).and_then(|s| s.info.as_ref())
    }

    /// `document.readyState`.
    pub fn ready_state(&self) -> DocumentReadiness {
        self.scripts.readiness
    }

    /// "Update the current document readiness" (§3.1.3): cambia el valor y dispara
    /// `readystatechange` en el documento.
    pub fn update_readiness(&mut self, readiness: DocumentReadiness, host: &mut dyn ScriptHost) {
        if self.scripts.readiness == readiness {
            return;
        }
        self.scripts.readiness = readiness;
        let root = self.root();
        self.fire_event(
            EventTarget::Node(root),
            "readystatechange",
            EventInit::default(),
            EventKind::Event,
            host,
        );
    }

    /// Cloning steps de `script` (§4.12.1): la copia hereda *already started*.
    pub(crate) fn clone_script_state(&mut self, node: NodeId, copy: NodeId) {
        if self
            .scripts
            .states
            .get(&node)
            .is_some_and(|s| s.already_started)
        {
            self.script_state_mut(copy).already_started = true;
        }
    }

    /// "Prepare the script element" (§4.12.1.1), pasos 1 a 30 y 31.5: los slots, el tipo y
    /// las opciones de búsqueda. Devuelve `None` si el algoritmo termina antes (no hay nada que
    /// buscar ni ejecutar); `scripting` es "scripting is enabled" para el elemento (paso 17).
    pub fn begin_prepare_script(&mut self, el: NodeId, scripting: bool) -> Option<ScriptInfo> {
        let element = self.element(el)?.clone();
        // 1.
        if self.script_state(el).already_started {
            return None;
        }
        // 2-3.
        let parser_document = std::mem::take(&mut self.script_state_mut(el).parser_inserted);
        // 4.
        let has_async = element.attr("async").is_some();
        if parser_document && !has_async {
            self.script_state_mut(el).force_async = true;
        }
        // 5.
        let source_text = self.child_text_content(el);
        // 6.
        let src = element
            .attrs
            .iter()
            .find(|a| a.ns.is_none() && a.name == "src")
            .map(|a| a.value.clone());
        if src.is_none() && source_text.is_empty() {
            return None;
        }
        // 7.
        if !self.is_connected(el) {
            return None;
        }
        // 8.
        let type_string: DomString = match (element.attr("type"), element.attr("language")) {
            (Some(""), _) | (None, Some("")) | (None, None) => "text/javascript".into(),
            (Some(t), _) => trim_ascii_whitespace(t).into(),
            (None, Some(l)) => format!("text/{l}").into(),
        };
        // 9-12.
        let ts = type_string.as_lossy_str();
        let type_ = if JAVASCRIPT_MIME_TYPES
            .iter()
            .any(|m| m.eq_ignore_ascii_case(ts))
        {
            ScriptType::Classic
        } else if ts.eq_ignore_ascii_case("module") {
            ScriptType::Module
        } else if ts.eq_ignore_ascii_case("importmap") {
            ScriptType::ImportMap
        } else {
            return None;
        };
        // 13.
        if parser_document {
            let state = self.script_state_mut(el);
            state.parser_inserted = true;
            state.force_async = false;
        }
        // 14-15 (el *preparation-time document* es siempre este documento).
        self.script_state_mut(el).already_started = true;
        // 17.
        if !scripting {
            return None;
        }
        // 18.
        let nomodule = element.attr("nomodule").is_some();
        if nomodule && type_ == ScriptType::Classic {
            return None;
        }
        // 19: sin Content Security Policy.
        // 20.
        if type_ == ScriptType::Classic
            && let (Some(event), Some(for_)) = (element.attr("event"), element.attr("for"))
        {
            if !trim_ascii_whitespace(for_).eq_ignore_ascii_case("window") {
                return None;
            }
            let event = trim_ascii_whitespace(event);
            if !event.eq_ignore_ascii_case("onload") && !event.eq_ignore_ascii_case("onload()") {
                return None;
            }
        }
        // 21.
        let encoding = element
            .attr("charset")
            .and_then(Encoding::for_label)
            .or_else(|| Encoding::for_label(self.character_set.as_lossy_str()))
            .map_or("UTF-8", |e| e.name());
        // 22-23.
        let crossorigin = match element.attr("crossorigin") {
            None => CorsSetting::NoCors,
            Some(v) if v.eq_ignore_ascii_case("use-credentials") => CorsSetting::UseCredentials,
            Some(_) => CorsSetting::Anonymous,
        };
        // 24-25.
        let nonce = element.attr("nonce").unwrap_or_default().into();
        let integrity = element.attr("integrity").unwrap_or_default().into();
        // 26-27.
        let referrer_policy = element
            .attr("referrerpolicy")
            .and_then(|v| {
                REFERRER_POLICIES
                    .iter()
                    .find(|p| p.eq_ignore_ascii_case(v))
                    .copied()
            })
            .unwrap_or("");
        let fetch_priority = match element.attr("fetchpriority") {
            Some(v) if v.eq_ignore_ascii_case("high") => FetchPriority::High,
            Some(v) if v.eq_ignore_ascii_case("low") => FetchPriority::Low,
            _ => FetchPriority::Auto,
        };
        // 31.5 / 32.1.
        let base_url = self.base_url();
        let url = src
            .as_ref()
            .and_then(|s| Url::parse(s.as_lossy_str(), Some(&base_url)).ok());
        let info = ScriptInfo {
            type_,
            type_string,
            source_text,
            src,
            url,
            is_async: has_async,
            defer: element.attr("defer").is_some(),
            nomodule,
            parser_inserted: parser_document,
            encoding,
            crossorigin,
            integrity,
            nonce,
            referrer_policy,
            fetch_priority,
            base_url,
        };
        self.script_state_mut(el).info = Some(info.clone());
        Some(info)
    }
}
