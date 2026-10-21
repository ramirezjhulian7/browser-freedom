//! `document.open()`, `write()`, `writeln()` y `close()` (HTML §8.4) sobre el `Parser` de
//! `bf-html`. Si el documento ya no tiene parser, `open` (o el `write` que lo llama) crea el
//! *script-created parser* con el documento. Mientras el parser trabaja sale del
//! [`crate::WebContext`]; los scripts que ejecuta ven el documento por el mismo intercambio
//! que el dispatch de eventos. Un `write` anidado dentro de un script escrito se ignora.

use bf_dom::events::{Event, ScriptHost as DomScriptHost};
use bf_dom::{Document, DomException, NodeId};
use bf_html::{ParseOptions, Parser, ScriptDisposition, ScriptHost};
use bf_infra::DomString;
use bf_js::vm::{Agent, JsResult, Throw};

use crate::dom::events::{JsHost, report_exception};
use crate::dom::state::state;
use crate::dom::throw_dom;
use crate::{DocIndex, DocumentOwner, ResourceKind, WebContextHandle};

/// Qué operación hacer con el parser.
#[derive(Debug, Clone)]
pub enum WriteOp {
    Open,
    Write(DomString),
    Writeln(DomString),
    Close,
}

/// El host del parser: los listeners con [`JsHost`] y los scripts inline con el agente.
struct ParserHost<'a> {
    inner: JsHost<'a>,
}

impl DomScriptHost for ParserHost<'_> {
    fn call_listener(&mut self, doc: &mut Document, handle: u64, event: &mut Event) -> bool {
        self.inner.call_listener(doc, handle, event)
    }
}

impl ScriptHost for ParserHost<'_> {
    fn prepare(&mut self, doc: &mut Parser, script: NodeId) -> ScriptDisposition {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return ScriptDisposition::Failed;
        };
        match (&info.src, &info.url) {
            (None, _) => ScriptDisposition::Ready,
            (Some(_), Some(url)) => {
                let ctx = self.inner.ctx;
                ctx.get_mut(self.inner.agent)
                    .hooks
                    .resource_needed(&url.serialize(false), ResourceKind::Script);
                ScriptDisposition::Pending
            }
            (Some(_), None) => ScriptDisposition::Failed,
        }
    }

    fn execute(&mut self, doc: &mut Parser, script: NodeId) {
        let Some(info) = doc.document().script_info(script).cloned() else {
            return;
        };
        if info.src.is_some() {
            return;
        }
        let ctx = self.inner.ctx;
        let i = self.inner.doc;
        let agent = &mut *self.inner.agent;
        std::mem::swap(ctx.get_mut(agent).doc_mut(i), doc.document_mut());
        let name = ctx.get(agent).url.serialize(false);
        let source = info.source_text.to_string();
        let r = agent
            .compile_script_text(&source, &name)
            .and_then(|code| agent.script_evaluation(code));
        if let Err(Throw::Value(e)) = r {
            report_exception(agent, &e);
        }
        std::mem::swap(ctx.get_mut(agent).doc_mut(i), doc.document_mut());
    }
}

/// Corre `op` sobre el parser del documento `doc`.
pub fn document_write(agent: &mut Agent, doc: DocIndex, op: WriteOp) -> JsResult<()> {
    if state(agent).parser_busy {
        return Ok(());
    }
    let ctx = WebContextHandle::current(agent);
    let slot = doc as usize;
    let owner = std::mem::replace(
        &mut ctx.get_mut(agent).documents[slot],
        DocumentOwner::Document(Box::default()),
    );
    let mut parser = match owner {
        DocumentOwner::Parser(p) => p,
        DocumentOwner::Document(d) => {
            // §8.4: en un documento XML todas lanzan; `close` sin parser no hace nada.
            let xml = !d.is_html;
            if xml || matches!(op, WriteOp::Close) {
                ctx.get_mut(agent).documents[slot] = DocumentOwner::Document(d);
                return if xml {
                    throw_dom(agent, DomException::InvalidStateError)
                } else {
                    Ok(())
                };
            }
            let mut p = Box::new(Parser::new(&ParseOptions {
                scripting: true,
                iframe_srcdoc: false,
            }));
            *p.document_mut() = *d;
            p
        }
    };
    state(agent).parser_busy = true;
    let r = {
        let mut host = ParserHost {
            inner: JsHost {
                agent,
                ctx,
                doc,
                wrapper: None,
            },
        };
        match &op {
            WriteOp::Open => parser.open(&mut host),
            WriteOp::Write(t) => parser.write(&mut host, t),
            WriteOp::Writeln(t) => parser.writeln(&mut host, t),
            WriteOp::Close => parser.close(&mut host),
        }
    };
    state(agent).parser_busy = false;
    ctx.get_mut(agent).documents[slot] = DocumentOwner::Parser(parser);
    crate::dom::mutated(agent);
    match r {
        Ok(()) => Ok(()),
        Err(e) => throw_dom(agent, e),
    }
}
