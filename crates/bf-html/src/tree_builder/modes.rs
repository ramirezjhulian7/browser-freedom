//! Insertion modes de 13.2.6.4, salvo "in body" (ver `in_body.rs`). Cada función transcribe su
//! subsección del spec en el mismo orden.

use bf_dom::NodeData;

use super::{Flow, Loc, Mode, Scope, TagToken, Tok, TreeBuilder, is_ascii_whitespace, quirks};
use crate::tokenizer::State;

/// Character token que es TAB, LF, FF, CR o SPACE.
fn is_ws(tok: &Tok) -> bool {
    matches!(tok, Tok::Char(c) if is_ascii_whitespace(*c))
}

fn is_start(tok: &Tok, names: &[&str]) -> bool {
    matches!(tok, Tok::Start(t) if names.contains(&t.name.as_lossy_str()))
}

fn is_end(tok: &Tok, names: &[&str]) -> bool {
    matches!(tok, Tok::End(t) if names.contains(&t.name.as_lossy_str()))
}

impl TreeBuilder {
    /// 13.2.6.4.1
    pub(super) fn initial(&mut self, tok: Tok) -> Flow {
        match tok {
            _ if is_ws(&tok) => {}
            Tok::Comment(data) => self.insert_comment(data, Some(self.document_loc())),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, Some(self.document_loc())),
            Tok::Doctype(d) => {
                if d.name.as_deref() != Some("html")
                    || d.public_id.is_some()
                    || d.system_id
                        .as_deref()
                        .is_some_and(|s| s != "about:legacy-compat")
                {
                    self.error();
                }
                let root = self.doc.root();
                let has_doctype_or_element = self.doc.children(root).any(|c| {
                    matches!(
                        self.doc.get(c).data,
                        NodeData::Doctype { .. } | NodeData::Element(_)
                    )
                });
                let node = self.doc.create(NodeData::Doctype {
                    name: d.name.clone().unwrap_or_default().into(),
                    public_id: d.public_id.clone().unwrap_or_default().into(),
                    system_id: d.system_id.clone().unwrap_or_default().into(),
                });
                if !has_doctype_or_element {
                    self.doc.append(root, node);
                }
                if !self.iframe_srcdoc {
                    self.set_quirks_mode(quirks::mode_for_doctype(&d));
                }
                self.mode = Mode::BeforeHtml;
            }
            _ => {
                if !self.iframe_srcdoc {
                    self.error();
                    self.set_quirks_mode(bf_dom::QuirksMode::Quirks);
                }
                self.mode = Mode::BeforeHtml;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.2
    pub(super) fn before_html(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Doctype(_) => self.error(),
            Tok::Comment(data) => self.insert_comment(data, Some(self.document_loc())),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, Some(self.document_loc())),
            _ if is_ws(&tok) => {}
            Tok::Start(ref t) if t.name == "html" => {
                let html = self.create_element_for_token(t, bf_infra::Namespace::Html);
                self.insert_element_at(html, Some(self.document_loc()));
                self.open.push(html);
                self.mode = Mode::BeforeHead;
            }
            Tok::End(ref t)
                if !matches!(t.name.as_lossy_str(), "head" | "body" | "html" | "br") =>
            {
                self.error();
            }
            _ => {
                let html = self
                    .create_element_for_token(&TagToken::named("html"), bf_infra::Namespace::Html);
                self.insert_element_at(html, Some(self.document_loc()));
                self.open.push(html);
                self.mode = Mode::BeforeHead;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.3
    pub(super) fn before_head(&mut self, tok: Tok) -> Flow {
        match tok {
            _ if is_ws(&tok) => {}
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::Start(ref t) if t.name == "head" => {
                let head = self.insert_html(t);
                self.head = Some(head);
                self.mode = Mode::InHead;
            }
            Tok::End(ref t)
                if !matches!(t.name.as_lossy_str(), "head" | "body" | "html" | "br") =>
            {
                self.error();
            }
            _ => {
                let head = self.insert_html(&TagToken::named("head"));
                self.head = Some(head);
                self.mode = Mode::InHead;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.4
    pub(super) fn in_head(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(c) if is_ascii_whitespace(c) => self.insert_char(c),
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::Start(ref t)
                if matches!(
                    t.name.as_lossy_str(),
                    "base" | "basefont" | "bgsound" | "link" | "meta"
                ) =>
            {
                // El cambio de encoding por `meta charset` no aplica: la entrada ya es texto.
                self.insert_html(t);
                self.pop();
            }
            Tok::Start(ref t) if t.name == "title" => self.generic_text_element(t, State::Rcdata),
            Tok::Start(ref t)
                if (t.name == "noscript" && self.scripting)
                    || matches!(t.name.as_lossy_str(), "noframes" | "style") =>
            {
                self.generic_text_element(t, State::Rawtext);
            }
            Tok::Start(ref t) if t.name == "noscript" => {
                self.insert_html(t);
                self.mode = Mode::InHeadNoscript;
            }
            Tok::Start(ref t) if t.name == "script" => {
                let script = self.create_element_for_token(t, bf_infra::Namespace::Html);
                self.insert_element_at(script, None);
                self.open.push(script);
                self.tokenizer.set_state(State::ScriptData);
                self.original_mode = self.mode;
                self.mode = Mode::Text;
            }
            Tok::End(ref t) if t.name == "head" => {
                self.pop();
                self.mode = Mode::AfterHead;
            }
            Tok::Start(ref t) if t.name == "template" => {
                self.push_marker();
                self.frameset_ok = false;
                self.mode = Mode::InTemplate;
                self.template_modes.push(Mode::InTemplate);
                let template = self.create_element_for_token(t, bf_infra::Namespace::Html);
                let location = self.appropriate_place(None);
                self.open.push(template);
                self.insert_element_at(template, Some(location));
            }
            Tok::End(ref t) if t.name == "template" => {
                if !self.stack_has_template() {
                    self.error();
                } else {
                    self.generate_implied_end_tags_thoroughly();
                    if !self.current_is_html("template") {
                        self.error();
                    }
                    self.pop_until_html("template");
                    self.clear_formatting_to_last_marker();
                    self.template_modes.pop();
                    self.reset_insertion_mode();
                }
            }
            Tok::Start(ref t) if t.name == "head" => self.error(),
            Tok::End(ref t) if !matches!(t.name.as_lossy_str(), "body" | "html" | "br") => {
                self.error()
            }
            _ => {
                self.pop();
                self.mode = Mode::AfterHead;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.5
    pub(super) fn in_head_noscript(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::End(ref t) if t.name == "noscript" => {
                self.pop();
                self.mode = Mode::InHead;
            }
            Tok::Comment(_) | Tok::Pi(_) => return self.in_head(tok),
            _ if is_ws(&tok)
                || is_start(
                    &tok,
                    &["basefont", "bgsound", "link", "meta", "noframes", "style"],
                ) =>
            {
                return self.in_head(tok);
            }
            Tok::Start(ref t) if matches!(t.name.as_lossy_str(), "head" | "noscript") => {
                self.error()
            }
            Tok::End(ref t) if t.name != "br" => self.error(),
            _ => {
                self.error();
                self.pop();
                self.mode = Mode::InHead;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.6
    pub(super) fn after_head(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(c) if is_ascii_whitespace(c) => self.insert_char(c),
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::Start(ref t) if t.name == "body" => {
                self.insert_html(t);
                self.frameset_ok = false;
                self.mode = Mode::InBody;
            }
            Tok::Start(ref t) if t.name == "frameset" => {
                self.insert_html(t);
                self.mode = Mode::InFrameset;
            }
            _ if is_start(
                &tok,
                &[
                    "base", "basefont", "bgsound", "link", "meta", "noframes", "script", "style",
                    "template", "title",
                ],
            ) =>
            {
                self.error();
                let head = self.head.expect("head element pointer");
                self.open.push(head);
                let flow = self.in_head(tok);
                self.remove_from_stack(head);
                return flow;
            }
            Tok::End(ref t) if t.name == "template" => return self.in_head(tok),
            Tok::Start(ref t) if t.name == "head" => self.error(),
            Tok::End(ref t) if !matches!(t.name.as_lossy_str(), "body" | "html" | "br") => {
                self.error()
            }
            _ => {
                self.insert_html(&TagToken::named("body"));
                self.frameset_ok = true;
                self.mode = Mode::InBody;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.8
    pub(super) fn text(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(c) => self.insert_char(c),
            Tok::Eof => {
                self.error();
                self.pop();
                self.mode = self.original_mode;
                return Flow::Reprocess(tok);
            }
            // "script" y cualquier otro end tag: sin ejecución de scripts, ambos casos se
            // reducen a sacar el elemento y volver al original insertion mode.
            Tok::End(_) => {
                self.pop();
                self.mode = self.original_mode;
            }
            _ => unreachable!("el tokenizador no emite este token en el modo text"),
        }
        Flow::Done
    }

    /// 13.2.6.4.9
    pub(super) fn in_table(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(_)
                if self.current_is_html_one_of(&[
                    "table", "tbody", "template", "tfoot", "thead", "tr",
                ]) =>
            {
                self.pending_table_chars.clear();
                self.original_mode = self.mode;
                self.mode = Mode::InTableText;
                return Flow::Reprocess(tok);
            }
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "caption" => {
                self.clear_stack_to_table_context();
                self.push_marker();
                self.insert_html(t);
                self.mode = Mode::InCaption;
            }
            Tok::Start(ref t) if t.name == "colgroup" => {
                self.clear_stack_to_table_context();
                self.insert_html(t);
                self.mode = Mode::InColumnGroup;
            }
            Tok::Start(ref t) if t.name == "col" => {
                self.clear_stack_to_table_context();
                self.insert_html(&TagToken::named("colgroup"));
                self.mode = Mode::InColumnGroup;
                return Flow::Reprocess(tok);
            }
            Tok::Start(ref t) if matches!(t.name.as_lossy_str(), "tbody" | "tfoot" | "thead") => {
                self.clear_stack_to_table_context();
                self.insert_html(t);
                self.mode = Mode::InTableBody;
            }
            Tok::Start(ref t) if matches!(t.name.as_lossy_str(), "td" | "th" | "tr") => {
                self.clear_stack_to_table_context();
                self.insert_html(&TagToken::named("tbody"));
                self.mode = Mode::InTableBody;
                return Flow::Reprocess(tok);
            }
            Tok::Start(ref t) if t.name == "table" => {
                self.error();
                if self.has_in_scope("table", Scope::Table) {
                    self.pop_until_html("table");
                    self.reset_insertion_mode();
                    return Flow::Reprocess(tok);
                }
            }
            Tok::End(ref t) if t.name == "table" => {
                if !self.has_in_scope("table", Scope::Table) {
                    self.error();
                } else {
                    self.pop_until_html("table");
                    self.reset_insertion_mode();
                }
            }
            _ if is_end(
                &tok,
                &[
                    "body", "caption", "col", "colgroup", "html", "tbody", "td", "tfoot", "th",
                    "thead", "tr",
                ],
            ) =>
            {
                self.error();
            }
            _ if is_start(&tok, &["style", "script", "template"])
                || is_end(&tok, &["template"]) =>
            {
                return self.in_head(tok);
            }
            Tok::Start(ref t)
                if t.name == "input"
                    && t.attr("type")
                        .is_some_and(|v| v.eq_ignore_ascii_case("hidden")) =>
            {
                self.error();
                self.insert_html(t);
                self.pop();
            }
            Tok::Start(ref t) if t.name == "form" => {
                self.error();
                if self.form.is_none() || self.parsing_template_contents() {
                    let form = self.insert_html(t);
                    if !self.parsing_template_contents() {
                        self.form = Some(form);
                    }
                    self.pop();
                }
            }
            Tok::Eof => return self.in_body(tok),
            _ => return self.in_table_anything_else(tok),
        }
        Flow::Done
    }

    /// "Anything else" de "in table": foster parenting alrededor de las reglas de "in body".
    fn in_table_anything_else(&mut self, tok: Tok) -> Flow {
        self.error();
        self.foster_parenting = true;
        let flow = self.in_body(tok);
        self.foster_parenting = false;
        flow
    }

    /// Clear the stack back to a table context.
    fn clear_stack_to_table_context(&mut self) {
        while !self.current_is_html_one_of(&["table", "template", "html"]) {
            self.pop();
        }
    }

    /// 13.2.6.4.10
    pub(super) fn in_table_text(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char('\0') => self.error(),
            Tok::Char(c) => self.pending_table_chars.push(c),
            _ => {
                let pending = std::mem::take(&mut self.pending_table_chars);
                if pending.chars().any(|c| !is_ascii_whitespace(c)) {
                    for c in pending.chars() {
                        let flow = self.in_table_anything_else(Tok::Char(c));
                        debug_assert!(matches!(flow, Flow::Done));
                    }
                } else {
                    self.insert_chars(&pending);
                }
                self.mode = self.original_mode;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.11
    pub(super) fn in_caption(&mut self, tok: Tok) -> Flow {
        let ends_caption = is_end(&tok, &["caption"]);
        let reprocess = is_start(
            &tok,
            &[
                "caption", "col", "colgroup", "tbody", "td", "tfoot", "th", "thead", "tr",
            ],
        ) || is_end(&tok, &["table"]);
        if ends_caption || reprocess {
            if !self.has_in_scope("caption", Scope::Table) {
                self.error();
                return Flow::Done;
            }
            self.generate_implied_end_tags(None);
            if !self.current_is_html("caption") {
                self.error();
            }
            self.pop_until_html("caption");
            self.clear_formatting_to_last_marker();
            self.mode = Mode::InTable;
            return if reprocess {
                Flow::Reprocess(tok)
            } else {
                Flow::Done
            };
        }
        if is_end(
            &tok,
            &[
                "body", "col", "colgroup", "html", "tbody", "td", "tfoot", "th", "thead", "tr",
            ],
        ) {
            self.error();
            return Flow::Done;
        }
        self.in_body(tok)
    }

    /// 13.2.6.4.12
    pub(super) fn in_column_group(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(c) if is_ascii_whitespace(c) => self.insert_char(c),
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::Start(ref t) if t.name == "col" => {
                self.insert_html(t);
                self.pop();
            }
            Tok::End(ref t) if t.name == "colgroup" => {
                if !self.current_is_html("colgroup") {
                    self.error();
                } else {
                    self.pop();
                    self.mode = Mode::InTable;
                }
            }
            Tok::End(ref t) if t.name == "col" => self.error(),
            _ if is_start(&tok, &["template"]) || is_end(&tok, &["template"]) => {
                return self.in_head(tok);
            }
            Tok::Eof => return self.in_body(tok),
            _ => {
                if !self.current_is_html("colgroup") {
                    self.error();
                } else {
                    self.pop();
                    self.mode = Mode::InTable;
                    return Flow::Reprocess(tok);
                }
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.13
    pub(super) fn in_table_body(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Start(ref t) if t.name == "tr" => {
                self.clear_stack_to_table_body_context();
                self.insert_html(t);
                self.mode = Mode::InRow;
            }
            Tok::Start(ref t) if matches!(t.name.as_lossy_str(), "th" | "td") => {
                self.error();
                self.clear_stack_to_table_body_context();
                self.insert_html(&TagToken::named("tr"));
                self.mode = Mode::InRow;
                return Flow::Reprocess(tok);
            }
            Tok::End(ref t) if matches!(t.name.as_lossy_str(), "tbody" | "tfoot" | "thead") => {
                if !self.has_in_scope(&t.name, Scope::Table) {
                    self.error();
                } else {
                    self.clear_stack_to_table_body_context();
                    self.pop();
                    self.mode = Mode::InTable;
                }
            }
            _ if is_start(
                &tok,
                &["caption", "col", "colgroup", "tbody", "tfoot", "thead"],
            ) || is_end(&tok, &["table"]) =>
            {
                let has_section = self.in_scope(
                    |n| self.is_html_one_of(n, &["tbody", "thead", "tfoot"]),
                    Scope::Table,
                );
                if !has_section {
                    self.error();
                } else {
                    self.clear_stack_to_table_body_context();
                    self.pop();
                    self.mode = Mode::InTable;
                    return Flow::Reprocess(tok);
                }
            }
            _ if is_end(
                &tok,
                &[
                    "body", "caption", "col", "colgroup", "html", "td", "th", "tr",
                ],
            ) =>
            {
                self.error();
            }
            _ => return self.in_table(tok),
        }
        Flow::Done
    }

    /// Clear the stack back to a table body context.
    fn clear_stack_to_table_body_context(&mut self) {
        while !self.current_is_html_one_of(&["tbody", "tfoot", "thead", "template", "html"]) {
            self.pop();
        }
    }

    /// 13.2.6.4.14
    pub(super) fn in_row(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Start(ref t) if matches!(t.name.as_lossy_str(), "th" | "td") => {
                self.clear_stack_to_table_row_context();
                self.insert_html(t);
                self.mode = Mode::InCell;
                self.push_marker();
            }
            Tok::End(ref t) if t.name == "tr" => {
                if !self.has_in_scope("tr", Scope::Table) {
                    self.error();
                } else {
                    self.clear_stack_to_table_row_context();
                    self.pop();
                    self.mode = Mode::InTableBody;
                }
            }
            _ if is_start(
                &tok,
                &[
                    "caption", "col", "colgroup", "tbody", "tfoot", "thead", "tr",
                ],
            ) || is_end(&tok, &["table"]) =>
            {
                if !self.has_in_scope("tr", Scope::Table) {
                    self.error();
                } else {
                    self.clear_stack_to_table_row_context();
                    self.pop();
                    self.mode = Mode::InTableBody;
                    return Flow::Reprocess(tok);
                }
            }
            Tok::End(ref t) if matches!(t.name.as_lossy_str(), "tbody" | "tfoot" | "thead") => {
                if !self.has_in_scope(&t.name, Scope::Table) {
                    self.error();
                } else if self.has_in_scope("tr", Scope::Table) {
                    self.clear_stack_to_table_row_context();
                    self.pop();
                    self.mode = Mode::InTableBody;
                    return Flow::Reprocess(tok);
                }
            }
            _ if is_end(
                &tok,
                &["body", "caption", "col", "colgroup", "html", "td", "th"],
            ) =>
            {
                self.error();
            }
            _ => return self.in_table(tok),
        }
        Flow::Done
    }

    /// Clear the stack back to a table row context.
    fn clear_stack_to_table_row_context(&mut self) {
        while !self.current_is_html_one_of(&["tr", "template", "html"]) {
            self.pop();
        }
    }

    /// 13.2.6.4.15
    pub(super) fn in_cell(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::End(ref t) if matches!(t.name.as_lossy_str(), "td" | "th") => {
                if !self.has_in_scope(&t.name, Scope::Table) {
                    self.error();
                } else {
                    self.generate_implied_end_tags(None);
                    if !self.current_is_html(&t.name) {
                        self.error();
                    }
                    self.pop_until_html(&t.name);
                    self.clear_formatting_to_last_marker();
                    self.mode = Mode::InRow;
                }
            }
            _ if is_start(
                &tok,
                &[
                    "caption", "col", "colgroup", "tbody", "td", "tfoot", "th", "thead", "tr",
                ],
            ) =>
            {
                debug_assert!(
                    self.in_scope(|n| self.is_html_one_of(n, &["td", "th"]), Scope::Table)
                );
                self.close_cell();
                return Flow::Reprocess(tok);
            }
            _ if is_end(&tok, &["body", "caption", "col", "colgroup", "html"]) => self.error(),
            Tok::End(ref t)
                if matches!(
                    t.name.as_lossy_str(),
                    "table" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                if !self.has_in_scope(&t.name, Scope::Table) {
                    self.error();
                } else {
                    self.close_cell();
                    return Flow::Reprocess(tok);
                }
            }
            _ => return self.in_body(tok),
        }
        Flow::Done
    }

    /// Close the cell.
    fn close_cell(&mut self) {
        self.generate_implied_end_tags(None);
        if !self.current_is_html_one_of(&["td", "th"]) {
            self.error();
        }
        self.pop_until(|tb, n| tb.is_html_one_of(n, &["td", "th"]));
        self.clear_formatting_to_last_marker();
        self.mode = Mode::InRow;
    }

    /// 13.2.6.4.16
    pub(super) fn in_template(&mut self, tok: Tok) -> Flow {
        let switch_to = |tb: &mut Self, mode: Mode, tok: Tok| {
            tb.template_modes.pop();
            tb.template_modes.push(mode);
            tb.mode = mode;
            Flow::Reprocess(tok)
        };
        match tok {
            Tok::Char(_) | Tok::Comment(_) | Tok::Pi(_) | Tok::Doctype(_) => self.in_body(tok),
            _ if is_start(
                &tok,
                &[
                    "base", "basefont", "bgsound", "link", "meta", "noframes", "script", "style",
                    "template", "title",
                ],
            ) || is_end(&tok, &["template"]) =>
            {
                self.in_head(tok)
            }
            _ if is_start(&tok, &["caption", "colgroup", "tbody", "tfoot", "thead"]) => {
                switch_to(self, Mode::InTable, tok)
            }
            _ if is_start(&tok, &["col"]) => switch_to(self, Mode::InColumnGroup, tok),
            _ if is_start(&tok, &["tr"]) => switch_to(self, Mode::InTableBody, tok),
            _ if is_start(&tok, &["td", "th"]) => switch_to(self, Mode::InRow, tok),
            Tok::Start(_) => switch_to(self, Mode::InBody, tok),
            Tok::End(_) => {
                self.error();
                Flow::Done
            }
            Tok::Eof => {
                if !self.stack_has_template() {
                    self.stopped = true;
                    return Flow::Done;
                }
                self.error();
                self.pop_until_html("template");
                self.clear_formatting_to_last_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
                Flow::Reprocess(tok)
            }
        }
    }

    /// 13.2.6.4.17
    pub(super) fn after_body(&mut self, tok: Tok) -> Flow {
        let html_loc = Loc {
            parent: self.open[0],
            before: None,
        };
        match tok {
            _ if is_ws(&tok) => return self.in_body(tok),
            Tok::Comment(data) => self.insert_comment(data, Some(html_loc)),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, Some(html_loc)),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::End(ref t) if t.name == "html" => {
                if self.context.is_some() {
                    self.error();
                } else {
                    self.mode = Mode::AfterAfterBody;
                }
            }
            Tok::Eof => self.stopped = true,
            _ => {
                self.error();
                self.mode = Mode::InBody;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.18
    pub(super) fn in_frameset(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(c) if is_ascii_whitespace(c) => self.insert_char(c),
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::Start(ref t) if t.name == "frameset" => {
                self.insert_html(t);
            }
            Tok::End(ref t) if t.name == "frameset" => {
                if self.current() == self.open[0] {
                    self.error();
                } else {
                    self.pop();
                    if self.context.is_none() && !self.current_is_html("frameset") {
                        self.mode = Mode::AfterFrameset;
                    }
                }
            }
            Tok::Start(ref t) if t.name == "frame" => {
                self.insert_html(t);
                self.pop();
            }
            Tok::Start(ref t) if t.name == "noframes" => return self.in_head(tok),
            Tok::Eof => {
                if self.current() != self.open[0] {
                    self.error();
                }
                self.stopped = true;
            }
            _ => self.error(),
        }
        Flow::Done
    }

    /// 13.2.6.4.19
    pub(super) fn after_frameset(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char(c) if is_ascii_whitespace(c) => self.insert_char(c),
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(ref t) if t.name == "html" => return self.in_body(tok),
            Tok::End(ref t) if t.name == "html" => self.mode = Mode::AfterAfterFrameset,
            Tok::Start(ref t) if t.name == "noframes" => return self.in_head(tok),
            Tok::Eof => self.stopped = true,
            _ => self.error(),
        }
        Flow::Done
    }

    /// 13.2.6.4.20
    pub(super) fn after_after_body(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Comment(data) => self.insert_comment(data, Some(self.document_loc())),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, Some(self.document_loc())),
            Tok::Doctype(_) => return self.in_body(tok),
            _ if is_ws(&tok) || is_start(&tok, &["html"]) => return self.in_body(tok),
            Tok::Eof => self.stopped = true,
            _ => {
                self.error();
                self.mode = Mode::InBody;
                return Flow::Reprocess(tok);
            }
        }
        Flow::Done
    }

    /// 13.2.6.4.21
    pub(super) fn after_after_frameset(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Comment(data) => self.insert_comment(data, Some(self.document_loc())),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, Some(self.document_loc())),
            Tok::Doctype(_) => return self.in_body(tok),
            _ if is_ws(&tok) || is_start(&tok, &["html"]) => return self.in_body(tok),
            Tok::Eof => self.stopped = true,
            Tok::Start(ref t) if t.name == "noframes" => return self.in_head(tok),
            _ => self.error(),
        }
        Flow::Done
    }
}
