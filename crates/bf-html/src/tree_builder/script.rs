//! Scripts en el parser HTML: el parser incremental ([`Parser`]) con pausa en `</script>`
//! (13.2.6.4.8), "prepare the script element" (§4.12.1.1) y "execute the script element"
//! (§4.12.1.3), `document.write()`/`writeln()`/`open()`/`close()` (§8.4) con el *insertion
//! point* y la reentrada del parser, y "the end" (13.2.7) con `DOMContentLoaded`, `load` y
//! `readyState`.
//!
//! El motor de script queda del otro lado de [`ScriptHost`]: el parser le avisa al preparar
//! cada `script` (para que busque el recurso o compile el inline) y le pide que lo ejecute
//! cuando corresponde. Las búsquedas que no terminan en el acto las completa el host con
//! [`Parser::script_fetched`]. El "event loop" de este módulo son las llamadas del host:
//! [`Parser::feed`], [`Parser::finish`], [`Parser::script_fetched`] y [`Parser::run_tasks`]
//! corren, al volver, las tareas encoladas (scripts async, eventos `error`, "the end").
//!
//! Se omiten: el parser especulativo, las hojas de estilo que bloquean scripts (paso 34.2 de
//! "prepare" y 13.2.7 paso 5), Content Security Policy, los scripts SVG y el *unload counter*.

use std::collections::VecDeque;

use bf_dom::events::{EventInit, EventTarget, NoScript};
use bf_dom::script::{DocumentReadiness, OnReady, ScriptResult, ScriptType};
use bf_dom::{Document, DomException, DomString, NodeId, QuirksMode};

use super::{ParseOptions, TreeBuilder};
use crate::tokenizer::ParseError;

/// Lo que responde el host al preparar un `script` (pasos 31-32 de "prepare the script
/// element": buscar el script externo o crear el inline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptDisposition {
    /// El resultado ya está: el inline compilado, o el externo ya descargado.
    Ready,
    /// La búsqueda sigue; el host llamará a [`Parser::script_fetched`].
    Pending,
    /// Falló (error de red o de compilación): el *result* es null y al ejecutarlo se dispara
    /// `error`.
    Failed,
}

/// El motor de script visto desde el parser.
pub trait ScriptHost: bf_dom::events::ScriptHost {
    /// "Prepare the script element": el parser ya clasificó el `script` (está en
    /// `doc.document().script_info(script)`); el host empieza a buscarlo o lo compila.
    fn prepare(&mut self, doc: &mut Parser, script: NodeId) -> ScriptDisposition {
        let _ = (doc, script);
        ScriptDisposition::Ready
    }

    /// Corre el script (paso 6 de "execute the script element"). Desde aquí el host puede
    /// llamar a [`Parser::write`] y compañía.
    fn execute(&mut self, doc: &mut Parser, script: NodeId) {
        let _ = (doc, script);
    }
}

impl ScriptHost for NoScript {}

/// En qué parte de "the end" (13.2.7) está el parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// Todavía parsea.
    NotStarted,
    /// Paso 5: ejecutando la *list of scripts that will execute when the document has
    /// finished parsing*.
    Deferred,
    /// Paso 7: esperando los scripts async y los "in order as soon as possible".
    AsSoonAsPossible,
    /// Paso 8: esperando lo que demora el evento `load`.
    Load,
    Complete,
}

/// Una tarea encolada en el event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Task {
    /// "Queue an element task ... to fire an event named error at el".
    FireError(NodeId),
    /// La búsqueda del script terminó ("mark as ready").
    MarkReady(NodeId, bool),
}

/// Estado del parser visto desde afuera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserStatus {
    /// Parseando: espera más entrada ([`Parser::feed`] / [`Parser::finish`]).
    Parsing,
    /// Bloqueado por el *pending parsing-blocking script*, que todavía no está listo.
    Blocked(NodeId),
    /// Terminó de parsear y espera scripts (defer/async) o el evento `load`.
    Finishing,
    /// `readyState` es "complete" y ya se disparó `load`.
    Complete,
}

/// Un documento HTML con su parser: el parser incremental de red, o el *script-created
/// parser* de `document.open()`.
pub struct Parser {
    b: TreeBuilder,
    opts: ParseOptions,
    end: End,
    tasks: VecDeque<Task>,
    /// Lo creó `document.open()`: solo `document.close()` le pone el EOF.
    script_created: bool,
    /// *Active parser was aborted*.
    aborted: bool,
    /// Cambia con cada `document.open()`: los pasos en curso del parser anterior se cortan.
    generation: u64,
}

impl Parser {
    /// Parser de red para un documento nuevo; la entrada llega con [`Parser::feed`] y termina
    /// con [`Parser::finish`].
    pub fn new(opts: &ParseOptions) -> Parser {
        Parser::from_builder(TreeBuilder::streaming(opts), opts)
    }

    /// Parser sobre la entrada completa (falta [`Parser::finish`] para procesarla).
    pub fn with_input(input: &str, opts: &ParseOptions) -> Parser {
        Parser::from_builder(TreeBuilder::new(input, opts), opts)
    }

    /// El "parser" de un documento que ya terminó de cargarse (o que se creó sin parser): no
    /// hay parser activo, y `document.write()` sin insertion point lo abre de nuevo
    /// (`document.open()`).
    pub fn from_document(doc: Document, opts: &ParseOptions) -> Parser {
        let mut b = TreeBuilder::new("", opts);
        b.doc = doc;
        b.stopped = true;
        Parser {
            b,
            opts: *opts,
            end: End::Complete,
            tasks: VecDeque::new(),
            script_created: false,
            aborted: false,
            generation: 0,
        }
    }

    fn from_builder(mut b: TreeBuilder, opts: &ParseOptions) -> Parser {
        // "Create and initialize a Document object": readiness "loading".
        b.doc.scripts_mut().readiness = DocumentReadiness::Loading;
        Parser {
            b,
            opts: *opts,
            end: End::NotStarted,
            tasks: VecDeque::new(),
            script_created: false,
            aborted: false,
            generation: 0,
        }
    }

    pub fn document(&self) -> &Document {
        &self.b.doc
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.b.doc
    }

    /// El documento y los errores de parseo.
    pub fn into_document(self) -> (Document, Vec<ParseError>) {
        self.b.finish()
    }

    /// Errores de parseo hasta ahora.
    pub fn errors(&self) -> Vec<ParseError> {
        let mut errors = self.b.tokenizer.errors().to_vec();
        errors.extend(self.b.errors.iter().cloned());
        errors.sort_by_key(|e| e.offset);
        errors
    }

    /// *Script nesting level*.
    pub fn script_nesting_level(&self) -> u32 {
        self.b.nesting
    }

    /// *Parser pause flag*.
    pub fn is_paused(&self) -> bool {
        self.b.paused
    }

    /// El *insertion point* (índice en la entrada del tokenizador), si está definido.
    pub fn insertion_point(&self) -> Option<usize> {
        self.b.insertion_point
    }

    /// ¿El documento tiene un *active parser* (uno que no terminó ni se abortó)?
    pub fn has_active_parser(&self) -> bool {
        !self.b.stopped && !self.aborted
    }

    pub fn status(&self) -> ParserStatus {
        if self.has_active_parser() {
            match self.b.doc.scripts().pending_parsing_blocking {
                Some(s) if !self.b.doc.script_state(s).ready_to_be_parser_executed => {
                    ParserStatus::Blocked(s)
                }
                _ => ParserStatus::Parsing,
            }
        } else if self.end == End::Complete {
            ParserStatus::Complete
        } else {
            ParserStatus::Finishing
        }
    }

    /// Agrega datos de red al final de la entrada y parsea lo que se pueda.
    pub fn feed(&mut self, input: &str, host: &mut dyn ScriptHost) {
        self.b.tokenizer.append(input.chars().map(u32::from));
        self.run_tasks(host);
    }

    /// Fin de los datos de red: parsea el resto y corre "the end" hasta donde se pueda sin
    /// esperar al host.
    pub fn finish(&mut self, host: &mut dyn ScriptHost) {
        if !self.script_created {
            self.b.tokenizer.close();
        }
        self.run_tasks(host);
    }

    /// "Abort a parser" (13.2.8): descarta la entrada pendiente, saca todos los nodos de la
    /// stack y deja `readyState` en "interactive".
    pub fn abort(&mut self, host: &mut dyn ScriptHost) {
        self.aborted = true;
        self.b.insertion_point = None;
        self.b
            .doc
            .update_readiness(DocumentReadiness::Interactive, host);
        let scripts = self.b.doc.scripts_mut();
        scripts.after_parsing.clear();
        scripts.as_soon_as_possible.clear();
        scripts.in_order.clear();
        scripts.pending_parsing_blocking = None;
        self.b.truncate_open(0);
    }

    /// La búsqueda de un script externo (o del grafo de un módulo) terminó: `ok` es `false`
    /// si falló. Es una tarea del event loop: después corre lo que haya quedado desbloqueado.
    pub fn script_fetched(&mut self, host: &mut dyn ScriptHost, script: NodeId, ok: bool) {
        self.tasks.push_back(Task::MarkReady(script, ok));
        self.run_tasks(host);
    }

    /// Corre lo pendiente del event loop: reanuda el parser si estaba bloqueado, corre las
    /// tareas encoladas y avanza "the end". No hace nada dentro de un script del parser.
    pub fn run_tasks(&mut self, host: &mut dyn ScriptHost) {
        if self.b.nesting > 0 {
            return;
        }
        loop {
            if self.has_active_parser() {
                self.run_parsing_blocking_scripts(host);
                if self.b.doc.scripts().pending_parsing_blocking.is_none() {
                    self.pump(host, false);
                }
            }
            if let Some(task) = self.tasks.pop_front() {
                match task {
                    Task::FireError(el) => self.fire_simple(host, el, "error"),
                    Task::MarkReady(el, ok) => self.mark_ready(host, el, ok),
                }
                continue;
            }
            self.advance_end(host);
            if self.tasks.is_empty() {
                break;
            }
        }
    }

    // --- Tokenización ---

    /// Corre el tokenizador y el tree builder hasta que falte entrada, el parser se pause o
    /// se bloquee; con `until_insertion_point`, sin pasar del insertion point (lo escrito por
    /// `document.write()`).
    fn pump(&mut self, host: &mut dyn ScriptHost, until_insertion_point: bool) {
        let generation = self.generation;
        loop {
            if self.generation != generation
                || self.b.stopped
                || self.b.paused
                || self.aborted
                || (self.b.nesting == 0 && self.b.doc.scripts().pending_parsing_blocking.is_some())
            {
                return;
            }
            let limit = if until_insertion_point {
                self.b.insertion_point
            } else {
                None
            };
            self.b.tokenizer.set_limit(limit);
            let Some(tok) = self.b.next_tok() else {
                return;
            };
            self.b.process_token(tok);
            if let Some(script) = self.b.script_end.take() {
                self.script_end_tag(host, script);
            }
            if self.b.stopped && self.generation == generation {
                self.stop_parsing(host);
            }
        }
    }

    /// 13.2.6.4.8, end tag "script", pasos 5 a 10.
    fn script_end_tag(&mut self, host: &mut dyn ScriptHost, script: NodeId) {
        let generation = self.generation;
        // 5.
        let old = self.b.insertion_point;
        self.b.saved_insertion_points.push(old);
        self.b.insertion_point = Some(self.b.tokenizer.offset());
        // 6-7.
        self.b.nesting += 1;
        self.prepare_script(host, script);
        if self.generation != generation {
            return;
        }
        // 8.
        self.b.nesting -= 1;
        if self.b.nesting == 0 {
            self.b.paused = false;
        }
        // 9.
        self.b.insertion_point = self.b.saved_insertion_points.pop().flatten();
        // 10.
        if self.b.doc.scripts().pending_parsing_blocking.is_some() {
            if self.b.nesting > 0 {
                self.b.paused = true;
            } else {
                self.run_parsing_blocking_scripts(host);
            }
        }
    }

    /// 13.2.6.4.8 paso 10, con el nesting level en cero: ejecuta el *pending parsing-blocking
    /// script* si ya está listo (si no, el parser queda bloqueado hasta que lo esté).
    fn run_parsing_blocking_scripts(&mut self, host: &mut dyn ScriptHost) {
        let generation = self.generation;
        while let Some(script) = self.b.doc.scripts().pending_parsing_blocking {
            if self.generation != generation
                || self.b.nesting > 0
                || !self.b.doc.script_state(script).ready_to_be_parser_executed
            {
                return;
            }
            self.b.doc.scripts_mut().pending_parsing_blocking = None;
            self.b.insertion_point = Some(self.b.tokenizer.offset());
            self.b.nesting += 1;
            self.execute_script(host, script);
            if self.generation != generation {
                return;
            }
            self.b.nesting -= 1;
            if self.b.nesting == 0 {
                self.b.paused = false;
            }
            self.b.insertion_point = None;
        }
    }

    // --- Scripts ---

    /// "Prepare the script element" (§4.12.1.1). La usa el parser al cerrar cada `script`; el
    /// host la llama para los `script` que se conectan al documento por el DOM.
    pub fn prepare_script(&mut self, host: &mut dyn ScriptHost, el: NodeId) {
        // 1-30.
        let Some(info) = self.b.doc.begin_prepare_script(el, self.b.scripting) else {
            return;
        };
        // 31.
        if let Some(src) = &info.src {
            if info.type_ == ScriptType::ImportMap || src.is_empty() || info.url.is_none() {
                self.tasks.push_back(Task::FireError(el));
                return;
            }
            let state = self.b.doc.script_state_mut(el);
            state.from_external_file = true;
            state.delaying_load_event = true;
        } else if info.type_ == ScriptType::Module {
            self.b.doc.script_state_mut(el).delaying_load_event = true;
        }
        let result = match host.prepare(self, el) {
            ScriptDisposition::Ready => Some(true),
            ScriptDisposition::Failed => Some(false),
            ScriptDisposition::Pending => None,
        };
        let state = self.b.doc.script_state(el);
        if (info.type_ == ScriptType::Classic && info.src.is_some())
            || info.type_ == ScriptType::Module
        {
            // 33.
            let scripts = self.b.doc.scripts_mut();
            let on_ready = if info.is_async || state.force_async {
                scripts.as_soon_as_possible.push(el);
                OnReady::AsSoonAsPossible
            } else if !state.parser_inserted {
                scripts.in_order.push(el);
                OnReady::InOrder
            } else if info.defer || info.type_ == ScriptType::Module {
                scripts.after_parsing.push(el);
                OnReady::ParserExecute
            } else {
                scripts.pending_parsing_blocking = Some(el);
                OnReady::ParserExecute
            };
            self.b.doc.script_state_mut(el).on_ready = Some(on_ready);
            if let Some(ok) = result {
                // La búsqueda terminó en el acto; lo que ejecuta scripts va como tarea.
                if on_ready == OnReady::ParserExecute {
                    self.mark_ready(host, el, ok);
                } else {
                    self.tasks.push_back(Task::MarkReady(el, ok));
                }
            }
        } else {
            // 34. Inline clásico o import map: el resultado ya está.
            self.b.doc.script_state_mut(el).result = if result == Some(false) {
                ScriptResult::Null
            } else {
                ScriptResult::Ready
            };
            self.execute_script(host, el);
        }
    }

    /// "Mark as ready" (§4.12.1.1).
    fn mark_ready(&mut self, host: &mut dyn ScriptHost, el: NodeId, ok: bool) {
        // 1.
        let state = self.b.doc.script_state_mut(el);
        state.result = if ok {
            ScriptResult::Ready
        } else {
            ScriptResult::Null
        };
        // 2-3.
        match state.on_ready.take() {
            Some(OnReady::AsSoonAsPossible) => {
                self.execute_script(host, el);
                self.b
                    .doc
                    .scripts_mut()
                    .as_soon_as_possible
                    .retain(|&s| s != el);
            }
            Some(OnReady::InOrder) => {
                if self.b.doc.scripts().in_order.first() == Some(&el) {
                    while let Some(&first) = self.b.doc.scripts().in_order.first() {
                        if self.b.doc.script_state(first).result == ScriptResult::Uninitialized {
                            break;
                        }
                        self.execute_script(host, first);
                        self.b.doc.scripts_mut().in_order.retain(|&s| s != first);
                    }
                }
            }
            Some(OnReady::ParserExecute) => {
                self.b.doc.script_state_mut(el).ready_to_be_parser_executed = true;
            }
            None => {}
        }
        // 4.
        self.b.doc.script_state_mut(el).delaying_load_event = false;
    }

    /// "Execute the script element" (§4.12.1.3).
    fn execute_script(&mut self, host: &mut dyn ScriptHost, el: NodeId) {
        let state = self.b.doc.script_state(el);
        let type_ = state.info.as_ref().map_or(ScriptType::Classic, |i| i.type_);
        // 1-2: el *preparation-time document* es siempre este documento.
        // 4.
        if state.result == ScriptResult::Null {
            self.fire_simple(host, el, "error");
            return;
        }
        // 5.
        let destructive = state.from_external_file || type_ == ScriptType::Module;
        if destructive {
            self.b.doc.scripts_mut().ignore_destructive_writes += 1;
        }
        // 6. `currentScript` es el elemento solo para scripts clásicos.
        let old = self.b.doc.scripts().current_script;
        self.b.doc.scripts_mut().current_script = (type_ == ScriptType::Classic).then_some(el);
        host.execute(self, el);
        self.b.doc.scripts_mut().current_script = old;
        // 7.
        if destructive {
            let scripts = self.b.doc.scripts_mut();
            scripts.ignore_destructive_writes = scripts.ignore_destructive_writes.saturating_sub(1);
        }
        // 8.
        if state.from_external_file {
            self.fire_simple(host, el, "load");
        }
    }

    /// Dispara un evento simple (sin burbujeo ni cancelación) en un elemento.
    fn fire_simple(&mut self, host: &mut dyn ScriptHost, el: NodeId, type_: &str) {
        self.b.doc.fire_event(
            EventTarget::Node(el),
            type_,
            EventInit::default(),
            Default::default(),
            host,
        );
    }

    // --- The end (13.2.7) ---

    /// "Stop parsing", pasos 1 a 4; el resto corre como tareas ([`Parser::advance_end`]).
    fn stop_parsing(&mut self, host: &mut dyn ScriptHost) {
        if self.end != End::NotStarted {
            return;
        }
        let generation = self.generation;
        self.end = End::Deferred;
        // 2.
        self.b.insertion_point = None;
        // 3.
        self.b
            .doc
            .update_readiness(DocumentReadiness::Interactive, host);
        if self.generation != generation {
            return;
        }
        // 4.
        self.b.truncate_open(0);
        self.b.doc.perform_microtask_checkpoint();
    }

    /// 13.2.7, pasos 5 a 9: lo que se pueda sin esperar al host.
    fn advance_end(&mut self, host: &mut dyn ScriptHost) {
        let generation = self.generation;
        loop {
            if self.generation != generation {
                return;
            }
            match self.end {
                End::NotStarted | End::Complete => return,
                End::Deferred => {
                    // 5.
                    while let Some(&first) = self.b.doc.scripts().after_parsing.first() {
                        if !self.b.doc.script_state(first).ready_to_be_parser_executed {
                            return;
                        }
                        self.execute_script(host, first);
                        if self.generation != generation {
                            return;
                        }
                        self.b
                            .doc
                            .scripts_mut()
                            .after_parsing
                            .retain(|&s| s != first);
                    }
                    // 6.
                    self.end = End::AsSoonAsPossible;
                    let root = self.b.doc.root();
                    self.b.doc.fire_event(
                        EventTarget::Node(root),
                        "DOMContentLoaded",
                        EventInit {
                            bubbles: true,
                            ..EventInit::default()
                        },
                        Default::default(),
                        host,
                    );
                }
                End::AsSoonAsPossible => {
                    // 7.
                    let scripts = self.b.doc.scripts();
                    if !scripts.as_soon_as_possible.is_empty() || !scripts.in_order.is_empty() {
                        return;
                    }
                    self.end = End::Load;
                }
                End::Load => {
                    // 8. Lo único que demora el `load` aquí son los scripts.
                    let delayed = {
                        let doc = &self.b.doc;
                        doc.inclusive_descendants(doc.root())
                            .into_iter()
                            .any(|n| doc.script_state(n).delaying_load_event)
                    };
                    if delayed {
                        return;
                    }
                    // 9.
                    self.end = End::Complete;
                    self.b
                        .doc
                        .update_readiness(DocumentReadiness::Complete, host);
                    if self.generation != generation {
                        return;
                    }
                    self.b.doc.fire_window_load(host);
                }
            }
        }
    }

    // --- document.open/write/close (§8.4) ---

    /// `document.open()`: "document open steps" (§8.4.1).
    pub fn open(&mut self, host: &mut dyn ScriptHost) -> Result<(), DomException> {
        let _ = host;
        // 1-2.
        if !self.b.doc.is_html || self.b.doc.scripts().throw_on_dynamic_markup_insertion > 0 {
            return Err(DomException::InvalidStateError);
        }
        // 5.
        if self.has_active_parser() && self.b.nesting > 0 {
            return Ok(());
        }
        // 7.
        if self.aborted {
            return Ok(());
        }
        // 8: el parser activo (sin scripts en curso) deja de cargar.
        // 9-10.
        self.b.doc.erase_all_event_listeners();
        // 11.
        let root = self.b.doc.root();
        self.b.doc.replace_all(None, root);
        // 15.
        self.b.doc.mode = QuirksMode::NoQuirks;
        // 16. Un parser nuevo, creado por script.
        self.generation += 1;
        let doc = std::mem::take(&mut self.b.doc);
        let mut b = TreeBuilder::streaming(&self.opts);
        b.doc = doc;
        self.b = b;
        self.script_created = true;
        self.end = End::NotStarted;
        self.tasks.clear();
        let scripts = self.b.doc.scripts_mut();
        scripts.after_parsing.clear();
        scripts.as_soon_as_possible.clear();
        scripts.in_order.clear();
        scripts.pending_parsing_blocking = None;
        // 17.
        self.b.insertion_point = Some(self.b.tokenizer.len());
        // 18.
        self.b
            .doc
            .update_readiness(DocumentReadiness::Loading, host);
        Ok(())
    }

    /// `document.close()` (§8.4.2).
    pub fn close(&mut self, host: &mut dyn ScriptHost) -> Result<(), DomException> {
        // 1-2.
        if !self.b.doc.is_html || self.b.doc.scripts().throw_on_dynamic_markup_insertion > 0 {
            return Err(DomException::InvalidStateError);
        }
        // 3.
        if !self.script_created || !self.has_active_parser() {
            return Ok(());
        }
        // 4.
        self.b.tokenizer.close();
        // 5.
        if self.b.doc.scripts().pending_parsing_blocking.is_some() {
            return Ok(());
        }
        // 6.
        self.pump(host, false);
        Ok(())
    }

    /// `document.write(...text)`: "document write steps" (§8.4.3).
    pub fn write(
        &mut self,
        host: &mut dyn ScriptHost,
        text: &DomString,
    ) -> Result<(), DomException> {
        // 3-4.
        if !self.b.doc.is_html || self.b.doc.scripts().throw_on_dynamic_markup_insertion > 0 {
            return Err(DomException::InvalidStateError);
        }
        // 5.
        if self.aborted {
            return Ok(());
        }
        // 6.
        if self.b.insertion_point.is_none() {
            if self.b.doc.scripts().ignore_destructive_writes > 0 {
                return Ok(());
            }
            self.open(host)?;
        }
        let Some(at) = self.b.insertion_point else {
            return Ok(());
        };
        // 7. Los insertion points guardados que están en `at` o después se corren.
        let len = self.b.tokenizer.insert(at, text.code_points());
        self.b.insertion_point = Some(at + len);
        for saved in self.b.saved_insertion_points.iter_mut().flatten() {
            if *saved >= at {
                *saved += len;
            }
        }
        // 8.
        if self.b.doc.scripts().pending_parsing_blocking.is_none() {
            self.pump(host, true);
        }
        Ok(())
    }

    /// `document.writeln(...text)` (§8.4.4): `write` con un salto de línea al final.
    pub fn writeln(
        &mut self,
        host: &mut dyn ScriptHost,
        text: &DomString,
    ) -> Result<(), DomException> {
        let mut line = text.clone();
        line.push('\n');
        self.write(host, &line)
    }
}
