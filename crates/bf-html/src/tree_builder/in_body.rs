//! 13.2.6.4.7 The "in body" insertion mode, con el adoption agency algorithm.

use bf_dom::{NodeData, QuirksMode};
use bf_infra::Namespace;

use super::{
    Flow, Formatting, Mode, Scope, TagToken, Tok, TreeBuilder, foreign, is_ascii_whitespace,
    is_special,
};
use crate::tokenizer::State;

const HEADINGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];

/// Elementos que pueden quedar abiertos al terminar el body sin que sea un parse error.
const BODY_END_OK: &[&str] = &[
    "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc", "tbody", "td", "tfoot",
    "th", "thead", "tr", "body", "html",
];

impl TreeBuilder {
    pub(super) fn in_body(&mut self, tok: Tok) -> Flow {
        match tok {
            Tok::Char('\0') => self.error(),
            Tok::Char(c) if is_ascii_whitespace(c) => {
                self.reconstruct_formatting();
                self.insert_char(c);
            }
            Tok::Char(c) => {
                self.reconstruct_formatting();
                self.insert_char(c);
                self.frameset_ok = false;
            }
            Tok::Comment(data) => self.insert_comment(data, None),
            Tok::Pi(pi) => self.insert_processing_instruction(pi, None),
            Tok::Doctype(_) => self.error(),
            Tok::Start(tag) => return self.in_body_start(tag),
            Tok::End(tag) => return self.in_body_end(tag),
            Tok::Eof => {
                if !self.template_modes.is_empty() {
                    return self.in_template(Tok::Eof);
                }
                self.check_open_elements_at_body_end();
                self.stopped = true;
            }
        }
        Flow::Done
    }

    fn check_open_elements_at_body_end(&mut self) {
        if self
            .open
            .iter()
            .any(|&n| !self.is_html_one_of(n, BODY_END_OK))
        {
            self.error();
        }
    }

    fn in_body_start(&mut self, mut tag: TagToken) -> Flow {
        match tag.name.as_lossy_str() {
            "html" => {
                self.error();
                if !self.stack_has_template() {
                    let html = self.open[0];
                    self.merge_attributes(html, &tag);
                }
            }
            "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style"
            | "template" | "title" => return self.in_head(Tok::Start(tag)),
            "body" => {
                self.error();
                if self.open.len() > 1
                    && self.is_html(self.open[1], "body")
                    && !self.stack_has_template()
                {
                    self.frameset_ok = false;
                    let body = self.open[1];
                    self.merge_attributes(body, &tag);
                }
            }
            "frameset" => {
                self.error();
                if self.open.len() > 1 && self.is_html(self.open[1], "body") && self.frameset_ok {
                    let body = self.open[1];
                    self.doc.remove(body);
                    self.truncate_open(1);
                    self.insert_html(&tag);
                    self.mode = Mode::InFrameset;
                }
            }
            "address" | "article" | "aside" | "blockquote" | "center" | "details" | "dialog"
            | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer" | "header"
            | "hgroup" | "main" | "menu" | "nav" | "ol" | "p" | "search" | "section"
            | "summary" | "ul" => {
                self.close_p_if_in_button_scope();
                self.insert_html(&tag);
            }
            name if HEADINGS.contains(&name) => {
                self.close_p_if_in_button_scope();
                if self.current_is_html_one_of(HEADINGS) {
                    self.error();
                    self.pop();
                }
                self.insert_html(&tag);
            }
            "pre" | "listing" => {
                self.close_p_if_in_button_scope();
                self.insert_html(&tag);
                self.skip_next_lf = true;
                self.frameset_ok = false;
            }
            "form" => {
                if self.form.is_some() && !self.parsing_template_contents() {
                    self.error();
                } else {
                    self.close_p_if_in_button_scope();
                    let form = self.insert_html(&tag);
                    if !self.parsing_template_contents() {
                        self.form = Some(form);
                    }
                }
            }
            "li" => {
                self.frameset_ok = false;
                for i in (0..self.open.len()).rev() {
                    let node = self.open[i];
                    if self.is_html(node, "li") {
                        self.generate_implied_end_tags(Some("li"));
                        if !self.current_is_html("li") {
                            self.error();
                        }
                        self.pop_until_html("li");
                        break;
                    }
                    if is_special(self.elem(node))
                        && !self.is_html_one_of(node, &["address", "div", "p"])
                    {
                        break;
                    }
                }
                self.close_p_if_in_button_scope();
                self.insert_html(&tag);
            }
            "dd" | "dt" => {
                self.frameset_ok = false;
                for i in (0..self.open.len()).rev() {
                    let node = self.open[i];
                    if let Some(name) = ["dd", "dt"].into_iter().find(|n| self.is_html(node, n)) {
                        self.generate_implied_end_tags(Some(name));
                        if !self.current_is_html(name) {
                            self.error();
                        }
                        self.pop_until_html(name);
                        break;
                    }
                    if is_special(self.elem(node))
                        && !self.is_html_one_of(node, &["address", "div", "p"])
                    {
                        break;
                    }
                }
                self.close_p_if_in_button_scope();
                self.insert_html(&tag);
            }
            "plaintext" => {
                self.close_p_if_in_button_scope();
                self.insert_html(&tag);
                self.tokenizer.set_state(State::Plaintext);
            }
            "button" => {
                if self.has_in_scope("button", Scope::Default) {
                    self.error();
                    self.generate_implied_end_tags(None);
                    self.pop_until_html("button");
                }
                self.reconstruct_formatting();
                self.insert_html(&tag);
                self.frameset_ok = false;
            }
            "a" => {
                let existing = self.formatting.iter().rev().find_map(|entry| match entry {
                    Formatting::Marker => Some(None),
                    Formatting::Element(n, _) if self.is_html(*n, "a") => Some(Some(*n)),
                    Formatting::Element(..) => None,
                });
                if let Some(Some(a)) = existing {
                    self.error();
                    self.adoption_agency("a");
                    if let Some(i) = self.formatting_index(a) {
                        self.formatting.remove(i);
                    }
                    self.remove_from_stack(a);
                }
                self.reconstruct_formatting();
                let element = self.insert_html(&tag);
                self.push_formatting(element, tag);
            }
            "b" | "big" | "code" | "em" | "font" | "i" | "s" | "small" | "strike" | "strong"
            | "tt" | "u" => {
                self.reconstruct_formatting();
                let element = self.insert_html(&tag);
                self.push_formatting(element, tag);
            }
            "nobr" => {
                self.reconstruct_formatting();
                if self.has_in_scope("nobr", Scope::Default) {
                    self.error();
                    self.adoption_agency("nobr");
                    self.reconstruct_formatting();
                }
                let element = self.insert_html(&tag);
                self.push_formatting(element, tag);
            }
            "applet" | "marquee" | "object" => {
                self.reconstruct_formatting();
                self.insert_html(&tag);
                self.push_marker();
                self.frameset_ok = false;
            }
            "table" => {
                if self.doc.mode != QuirksMode::Quirks {
                    self.close_p_if_in_button_scope();
                }
                self.insert_html(&tag);
                self.frameset_ok = false;
                self.mode = Mode::InTable;
            }
            "area" | "br" | "embed" | "img" | "keygen" | "wbr" => {
                self.reconstruct_formatting();
                self.insert_html(&tag);
                self.pop();
                self.frameset_ok = false;
            }
            "input" => {
                if self.context.is_some_and(|c| self.is_html(c, "select")) {
                    self.error();
                    return Flow::Done;
                }
                if self.has_in_scope("select", Scope::Default) {
                    self.error();
                    self.pop_until_html("select");
                }
                self.reconstruct_formatting();
                self.insert_html(&tag);
                self.pop();
                if !tag
                    .attr("type")
                    .is_some_and(|v| v.eq_ignore_ascii_case("hidden"))
                {
                    self.frameset_ok = false;
                }
            }
            "param" | "source" | "track" => {
                self.insert_html(&tag);
                self.pop();
            }
            "hr" => {
                self.close_p_if_in_button_scope();
                if self.has_in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(None);
                    if self.has_in_scope("option", Scope::Default)
                        || self.has_in_scope("optgroup", Scope::Default)
                    {
                        self.error();
                    }
                }
                self.insert_html(&tag);
                self.pop();
                self.frameset_ok = false;
            }
            "image" => {
                self.error();
                tag.name = "img".into();
                return Flow::Reprocess(Tok::Start(tag));
            }
            "textarea" => {
                self.insert_html(&tag);
                self.skip_next_lf = true;
                self.tokenizer.set_state(State::Rcdata);
                self.original_mode = self.mode;
                self.frameset_ok = false;
                self.mode = Mode::Text;
            }
            "xmp" => {
                self.close_p_if_in_button_scope();
                self.reconstruct_formatting();
                self.frameset_ok = false;
                self.generic_text_element(&tag, State::Rawtext);
            }
            "iframe" => {
                self.frameset_ok = false;
                self.generic_text_element(&tag, State::Rawtext);
            }
            "noembed" => self.generic_text_element(&tag, State::Rawtext),
            "noscript" if self.scripting => self.generic_text_element(&tag, State::Rawtext),
            "select" => {
                if self.context.is_some_and(|c| self.is_html(c, "select")) {
                    self.error();
                } else if self.has_in_scope("select", Scope::Default) {
                    self.error();
                    self.pop_until_html("select");
                } else {
                    self.reconstruct_formatting();
                    self.insert_html(&tag);
                    self.frameset_ok = false;
                }
            }
            "option" => {
                if self.has_in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(Some("optgroup"));
                    if self.has_in_scope("option", Scope::Default) {
                        self.error();
                    }
                } else if self.current_is_html("option") {
                    self.pop();
                }
                self.reconstruct_formatting();
                self.insert_html(&tag);
            }
            "optgroup" => {
                if self.has_in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(None);
                    if self.has_in_scope("option", Scope::Default)
                        || self.has_in_scope("optgroup", Scope::Default)
                    {
                        self.error();
                    }
                } else if self.current_is_html("option") {
                    self.pop();
                }
                self.reconstruct_formatting();
                self.insert_html(&tag);
            }
            "rb" | "rtc" => {
                if self.has_in_scope("ruby", Scope::Default) {
                    self.generate_implied_end_tags(None);
                    if !self.current_is_html("ruby") {
                        self.error();
                    }
                }
                self.insert_html(&tag);
            }
            "rp" | "rt" => {
                if self.has_in_scope("ruby", Scope::Default) {
                    self.generate_implied_end_tags(Some("rtc"));
                    if !self.current_is_html_one_of(&["rtc", "ruby"]) {
                        self.error();
                    }
                }
                self.insert_html(&tag);
            }
            "math" => {
                self.reconstruct_formatting();
                foreign::adjust_mathml_attributes(&mut tag);
                foreign::adjust_foreign_attributes(&mut tag);
                self.insert_foreign(&tag, Namespace::MathMl);
                if tag.self_closing {
                    self.pop();
                }
            }
            "svg" => {
                self.reconstruct_formatting();
                foreign::adjust_svg_attributes(&mut tag);
                foreign::adjust_foreign_attributes(&mut tag);
                self.insert_foreign(&tag, Namespace::Svg);
                if tag.self_closing {
                    self.pop();
                }
            }
            "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th"
            | "thead" | "tr" => self.error(),
            _ => {
                self.reconstruct_formatting();
                self.insert_html(&tag);
            }
        }
        Flow::Done
    }

    fn in_body_end(&mut self, tag: TagToken) -> Flow {
        match tag.name.as_lossy_str() {
            "template" => return self.in_head(Tok::End(tag)),
            "body" | "html" => {
                if !self.has_in_scope("body", Scope::Default) {
                    self.error();
                    return Flow::Done;
                }
                self.check_open_elements_at_body_end();
                self.mode = Mode::AfterBody;
                if tag.name == "html" {
                    return Flow::Reprocess(Tok::End(tag));
                }
            }
            "address" | "article" | "aside" | "blockquote" | "button" | "center" | "details"
            | "dialog" | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer"
            | "header" | "hgroup" | "listing" | "main" | "menu" | "nav" | "ol" | "pre"
            | "search" | "section" | "select" | "summary" | "ul" => {
                if !self.has_in_scope(&tag.name, Scope::Default) {
                    self.error();
                } else {
                    self.generate_implied_end_tags(None);
                    if !self.current_is_html(&tag.name) {
                        self.error();
                    }
                    self.pop_until_html(&tag.name);
                }
            }
            "form" => {
                if !self.parsing_template_contents() {
                    let node = self.form.take();
                    let Some(node) = node.filter(|&n| self.has_node_in_scope(n)) else {
                        self.error();
                        return Flow::Done;
                    };
                    self.generate_implied_end_tags(None);
                    if self.current() != node {
                        self.error();
                    }
                    self.remove_from_stack(node);
                } else {
                    if !self.has_in_scope("form", Scope::Default) {
                        self.error();
                        return Flow::Done;
                    }
                    self.generate_implied_end_tags(None);
                    if !self.current_is_html("form") {
                        self.error();
                    }
                    self.pop_until_html("form");
                }
            }
            "p" => {
                if !self.has_in_scope("p", Scope::Button) {
                    self.error();
                    self.insert_html(&TagToken::named("p"));
                }
                self.close_p();
            }
            "li" => {
                if !self.has_in_scope("li", Scope::ListItem) {
                    self.error();
                } else {
                    self.generate_implied_end_tags(Some("li"));
                    if !self.current_is_html("li") {
                        self.error();
                    }
                    self.pop_until_html("li");
                }
            }
            "dd" | "dt" => {
                if !self.has_in_scope(&tag.name, Scope::Default) {
                    self.error();
                } else {
                    self.generate_implied_end_tags(Some(&tag.name));
                    if !self.current_is_html(&tag.name) {
                        self.error();
                    }
                    self.pop_until_html(&tag.name);
                }
            }
            name if HEADINGS.contains(&name) => {
                if !self.in_scope(|n| self.is_html_one_of(n, HEADINGS), Scope::Default) {
                    self.error();
                } else {
                    self.generate_implied_end_tags(None);
                    if !self.current_is_html(&tag.name) {
                        self.error();
                    }
                    self.pop_until(|tb, n| tb.is_html_one_of(n, HEADINGS));
                }
            }
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small"
            | "strike" | "strong" | "tt" | "u" => self.adoption_agency(&tag.name),
            "applet" | "marquee" | "object" => {
                if !self.has_in_scope(&tag.name, Scope::Default) {
                    self.error();
                } else {
                    self.generate_implied_end_tags(None);
                    if !self.current_is_html(&tag.name) {
                        self.error();
                    }
                    self.pop_until_html(&tag.name);
                    self.clear_formatting_to_last_marker();
                }
            }
            "br" => {
                // Se descartan los atributos y se trata como un start tag "br".
                self.error();
                return self.in_body_start(TagToken::named("br"));
            }
            // Incluye "sarcasm" ("take a deep breath").
            _ => self.any_other_end_tag(&tag.name),
        }
        Flow::Done
    }

    /// Para los start tags `html` y `body`: agrega los atributos que el elemento no tenga.
    fn merge_attributes(&mut self, element: bf_dom::NodeId, tag: &TagToken) {
        let NodeData::Element(el) = &mut self.doc.get_mut(element).data else {
            return;
        };
        for attr in &tag.attrs {
            if !el
                .attrs
                .iter()
                .any(|a| a.name == attr.name && a.ns == attr.ns)
            {
                el.attrs.push(attr.clone());
            }
        }
    }

    fn close_p_if_in_button_scope(&mut self) {
        if self.has_in_scope("p", Scope::Button) {
            self.close_p();
        }
    }

    /// Close a p element.
    fn close_p(&mut self) {
        self.generate_implied_end_tags(Some("p"));
        if !self.current_is_html("p") {
            self.error();
        }
        self.pop_until_html("p");
    }

    /// "Any other end tag" de "in body".
    fn any_other_end_tag(&mut self, name: &str) {
        for i in (0..self.open.len()).rev() {
            let node = self.open[i];
            if self.is_html(node, name) {
                self.generate_implied_end_tags(Some(name));
                if node != self.current() {
                    self.error();
                }
                self.truncate_open(i);
                return;
            }
            if is_special(self.elem(node)) {
                self.error();
                return;
            }
        }
    }

    /// The adoption agency algorithm (parser sanitizer configuration null).
    fn adoption_agency(&mut self, subject: &str) {
        let current = self.current();
        if self.is_html(current, subject) && self.formatting_index(current).is_none() {
            self.pop();
            return;
        }
        for _outer in 0..8 {
            // formattingElement: el último de la lista, después del último marker, con ese nombre.
            let mut found = None;
            for (i, entry) in self.formatting.iter().enumerate().rev() {
                match entry {
                    Formatting::Marker => break,
                    Formatting::Element(n, tag) if self.is_html(*n, subject) => {
                        found = Some((i, *n, tag.clone()));
                        break;
                    }
                    Formatting::Element(..) => {}
                }
            }
            let Some((fe_index, formatting_element, fe_tag)) = found else {
                self.any_other_end_tag(subject);
                return;
            };
            let Some(fe_stack) = self.open.iter().rposition(|&n| n == formatting_element) else {
                self.error();
                self.formatting.remove(fe_index);
                return;
            };
            if !self.has_node_in_scope(formatting_element) {
                self.error();
                return;
            }
            if formatting_element != self.current() {
                self.error();
            }
            let furthest =
                (fe_stack + 1..self.open.len()).find(|&i| is_special(self.elem(self.open[i])));
            let Some(fb_stack) = furthest else {
                self.truncate_open(fe_stack);
                self.formatting.remove(fe_index);
                return;
            };
            let furthest_block = self.open[fb_stack];
            let common_ancestor = self.open[fe_stack - 1];
            // El bookmark es el índice donde se insertará el nuevo elemento, contado con
            // formattingElement todavía en la lista.
            let mut bookmark = fe_index;
            let mut bookmark_moved = false;
            let mut node_stack = fb_stack;
            let mut last_node = furthest_block;
            let mut inner = 0;
            loop {
                inner += 1;
                // El elemento inmediatamente arriba de node (o el que estaba arriba antes de
                // quitar node de la stack): en ambos casos es el índice anterior.
                node_stack -= 1;
                let node = self.open[node_stack];
                if node == formatting_element {
                    break;
                }
                let mut node_fmt = self.formatting_index(node);
                if inner > 3
                    && let Some(i) = node_fmt
                {
                    self.formatting.remove(i);
                    if i < bookmark {
                        bookmark -= 1;
                    }
                    node_fmt = None;
                }
                let Some(i) = node_fmt else {
                    let removed = self.open.remove(node_stack);
                    self.doc.option_popped(removed);
                    continue;
                };
                let Formatting::Element(_, tag) = &self.formatting[i] else {
                    unreachable!("formatting_index solo devuelve elementos");
                };
                let tag = tag.clone();
                let new = self.create_element_for_token(&tag, Namespace::Html);
                self.formatting[i] = Formatting::Element(new, tag);
                self.open[node_stack] = new;
                if !bookmark_moved {
                    bookmark = i + 1;
                    bookmark_moved = true;
                }
                self.doc.append(new, last_node);
                last_node = new;
            }

            let loc = self.appropriate_place(Some(common_ancestor));
            self.doc.remove(last_node);
            let target_is_document_with_element =
                matches!(self.doc.get(loc.parent).data, NodeData::Document)
                    && self.has_element_child(loc.parent);
            if !self.is_host_including_inclusive_ancestor(last_node, loc.parent)
                && !target_is_document_with_element
                && loc
                    .before
                    .is_none_or(|b| self.doc.parent(b) == Some(loc.parent))
            {
                self.doc.insert_before(loc.parent, last_node, loc.before);
            }

            let new_element = self.create_element_for_token(&fe_tag, Namespace::Html);
            self.formatting
                .insert(bookmark, Formatting::Element(new_element, fe_tag));
            if let Some(i) = self
                .formatting
                .iter()
                .position(|e| matches!(e, Formatting::Element(n, _) if *n == formatting_element))
            {
                self.formatting.remove(i);
            }
            self.remove_from_stack(formatting_element);
            let fb_pos = self
                .open
                .iter()
                .rposition(|&n| n == furthest_block)
                .expect("furthestBlock sigue en la stack");
            self.open.insert(fb_pos + 1, new_element);

            let children: Vec<_> = self.doc.children(furthest_block).collect();
            for child in children {
                self.doc.append(new_element, child);
            }
            self.doc.append(furthest_block, new_element);
        }
    }
}
