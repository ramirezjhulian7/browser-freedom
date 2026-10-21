//! Comportamiento de `select`, `option` y `selectedcontent` que observa el parser
//! (WHATWG HTML 4.10.7 a 4.10.11): selectedness de `option`, su *cached nearest ancestor select*
//! y el clonado del `option` seleccionado en los `selectedcontent`.
//!
//! Solo se implementa lo que dispara la inserción y remoción de nodos y el hook del parser
//! ([`Document::option_popped`]). Los cambios de atributos (`selected`, `disabled`, `multiple`,
//! `size`) después de crear el elemento todavía no disparan nada.

use crate::{Document, NodeId};

#[derive(Debug, Clone, Default)]
pub(crate) enum ElementState {
    #[default]
    None,
    Option {
        selectedness: bool,
        dirtiness: bool,
        cached_select: Option<NodeId>,
    },
    SelectedContent {
        disabled: bool,
    },
    /// `input`: checkedness y dirty checkedness (HTML §4.10.5).
    Input {
        checked: bool,
        dirty: bool,
    },
}

/// Microtasks encolados por los removing steps de `option`.
///
/// TODO(event-loop): esta cola es provisional; cuando exista el event loop real (con bf-js) los
/// microtasks se encolarán allí.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Microtask {
    UpdateSelectedContents(NodeId),
}

impl Document {
    fn is_html_element(&self, id: NodeId, name: &str) -> bool {
        self.element(id).is_some_and(|e| e.is_html(name))
    }

    fn has_attr(&self, id: NodeId, name: &str) -> bool {
        self.element(id).is_some_and(|e| e.attr(name).is_some())
    }

    pub(crate) fn init_element_state(&mut self, id: NodeId) {
        let state = if self.is_html_element(id, "option") {
            // "When the element is created, its selectedness must be set to true if the
            // element has a selected attribute."
            ElementState::Option {
                selectedness: self.has_attr(id, "selected"),
                dirtiness: false,
                cached_select: None,
            }
        } else if self.is_html_element(id, "selectedcontent") {
            ElementState::SelectedContent { disabled: false }
        } else if self.is_html_element(id, "input") {
            // "The checkedness of an input element ... when created": su atributo `checked`.
            ElementState::Input {
                checked: self.has_attr(id, "checked"),
                dirty: false,
            }
        } else {
            ElementState::None
        };
        self.get_mut(id).state = state;
        // E1: un `details` creado con `open` (parser, clon) también dispara `toggle`: sus
        // atributos se agregan con los "attribute change steps" (HTML §4.11.1).
        if self.is_html_element(id, "details") && self.has_attr(id, "open") {
            self.details_open_changed(id, false);
        }
    }

    /// Cloning steps de `option`: la copia hereda selectedness y dirtiness.
    pub(crate) fn clone_element_state(&mut self, from: NodeId, to: NodeId) {
        if let ElementState::Option {
            selectedness,
            dirtiness,
            ..
        } = self.get(from).state
            && let ElementState::Option {
                selectedness: s,
                dirtiness: d,
                ..
            } = &mut self.get_mut(to).state
        {
            *s = selectedness;
            *d = dirtiness;
        }
        // Cloning steps de `input`: checkedness y dirty checkedness.
        if let ElementState::Input { checked, dirty } = self.get(from).state
            && let ElementState::Input {
                checked: c,
                dirty: d,
            } = &mut self.get_mut(to).state
        {
            *c = checked;
            *d = dirty;
        }
    }

    fn has_steps(&self, id: NodeId) -> bool {
        matches!(
            self.get(id).state,
            ElementState::Option { .. } | ElementState::SelectedContent { .. }
        )
    }

    /// "Attribute change steps" de `input` (`checked`) y `option` (`selected`): si el estado
    /// no está *dirty*, sigue a la presencia del atributo (HTML §4.10.5 y §4.10.10).
    pub(crate) fn attribute_changed(&mut self, el: NodeId, local: &bf_infra::DomString) {
        let present = |doc: &Self, name: &str| doc.has_attr(el, name);
        match self.get(el).state {
            ElementState::Input { dirty: false, .. } if *local == "checked" => {
                let value = present(self, "checked");
                if let ElementState::Input { checked, .. } = &mut self.get_mut(el).state {
                    *checked = value;
                }
            }
            ElementState::Option {
                dirtiness: false, ..
            } if *local == "selected" => {
                let value = present(self, "selected");
                self.set_option_selectedness(el, value);
            }
            _ => {}
        }
    }

    /// E1: el "reset algorithm" de `input` (checkedness del atributo `checked`, sin dirty) y
    /// de `select` (selectedness de los atributos `selected`, HTML §4.10.21.6).
    pub fn reset_form_control(&mut self, el: NodeId) {
        let has = |doc: &Self, n: NodeId, name: &str| {
            doc.element(n).is_some_and(|e| e.attr(name).is_some())
        };
        let checked_attr = has(self, el, "checked");
        if let ElementState::Input { checked, dirty } = &mut self.get_mut(el).state {
            *checked = checked_attr;
            *dirty = false;
            return;
        }
        if self.is_html_element(el, "select") {
            for o in self.list_of_options(el) {
                let value = has(self, o, "selected");
                if let ElementState::Option {
                    selectedness,
                    dirtiness,
                    ..
                } = &mut self.get_mut(o).state
                {
                    *selectedness = value;
                    *dirtiness = false;
                }
            }
            self.selectedness_setting(el);
        }
    }

    /// Checkedness de un `input` (`false` si no lo es).
    pub fn checkedness(&self, input: NodeId) -> bool {
        matches!(
            self.get(input).state,
            ElementState::Input { checked: true, .. }
        )
    }

    /// Cambia la checkedness (marca dirty); al marcar un radio se desmarca el resto de su
    /// grupo (HTML §4.10.5.1.16).
    pub fn set_checkedness(&mut self, input: NodeId, value: bool) {
        if let ElementState::Input { checked, dirty } = &mut self.get_mut(input).state {
            *checked = value;
            *dirty = true;
        } else {
            return;
        }
        if value {
            for other in self.radio_group(input) {
                if other != input
                    && let ElementState::Input { checked, .. } = &mut self.get_mut(other).state
                {
                    *checked = false;
                }
            }
        }
    }

    /// El "radio button group" de un radio: los `input type=radio` del mismo árbol con el
    /// mismo `name` no vacío (sin considerar el form owner).
    pub fn radio_group(&self, input: NodeId) -> Vec<NodeId> {
        let is_radio = |n: NodeId| {
            self.element(n).is_some_and(|e| {
                e.is_html("input")
                    && e.attr("type")
                        .is_some_and(|t| t.eq_ignore_ascii_case("radio"))
            })
        };
        if !is_radio(input) {
            return Vec::new();
        }
        let Some(name) = self
            .element(input)
            .and_then(|e| e.attr("name"))
            .filter(|n| !n.is_empty())
        else {
            return vec![input];
        };
        let root = self.tree_root(input);
        self.inclusive_descendants(root)
            .into_iter()
            .filter(|&n| is_radio(n) && self.element(n).and_then(|e| e.attr("name")) == Some(name))
            .collect()
    }

    /// Insertion steps y post-connection steps (DOM "insert", pasos 7.7 y 11).
    pub(crate) fn run_insertion_steps(&mut self, node: NodeId) {
        let nodes = self.inclusive_descendants(node);
        if !nodes.iter().any(|&n| self.has_steps(n)) {
            return;
        }
        for &n in &nodes {
            match self.get(n).state {
                // Los selects devueltos se ignoran a propósito: los post-connection steps hacen
                // la actualización.
                ElementState::Option { .. } => {
                    self.update_option_nearest_select(n);
                }
                ElementState::SelectedContent { .. } => {
                    self.recalculate_selectedcontent_disabledness(n);
                }
                ElementState::None | ElementState::Input { .. } => {}
            }
        }
        if !self.is_connected(node) {
            return;
        }
        for &n in &nodes {
            match self.get(n).state {
                ElementState::Option { .. } => self.update_selectedcontents_for_option(n),
                ElementState::SelectedContent { .. } => {
                    let select = self.recalculate_selectedcontent_disabledness(n);
                    if let Some(select) = select
                        && !self.selectedcontent_disabled(n)
                        && !self.has_attr(select, "multiple")
                    {
                        self.update_selectedcontent(select, n);
                    }
                }
                ElementState::None | ElementState::Input { .. } => {}
            }
        }
    }

    /// Removing steps de `option`.
    pub(crate) fn run_removing_steps(&mut self, node: NodeId) {
        let nodes = self.inclusive_descendants(node);
        for n in nodes {
            if matches!(self.get(n).state, ElementState::Option { .. }) {
                for select in self.update_option_nearest_select(n) {
                    self.microtasks
                        .push(Microtask::UpdateSelectedContents(select));
                }
            }
        }
    }

    /// Hook de 4.10.10: "When an option element option is popped off the stack of open
    /// elements of an HTML parser or XML parser, update descendant selectedcontent elements for
    /// an option given option." El parser lo llama para cada elemento que saca de la stack.
    pub fn option_popped(&mut self, node: NodeId) {
        if matches!(self.get(node).state, ElementState::Option { .. }) {
            self.update_selectedcontents_for_option(node);
        }
    }

    /// Ejecuta los microtasks pendientes.
    pub fn perform_microtask_checkpoint(&mut self) {
        while !self.microtasks.is_empty() {
            for task in std::mem::take(&mut self.microtasks) {
                match task {
                    Microtask::UpdateSelectedContents(select) => {
                        self.update_select_selectedcontents(select);
                    }
                }
            }
        }
    }

    /// Selectedness de un `option` (`false` si no lo es).
    pub fn option_selectedness(&self, option: NodeId) -> bool {
        matches!(
            self.get(option).state,
            ElementState::Option {
                selectedness: true,
                ..
            }
        )
    }

    /// `option.selected = value` (HTML §4.10.10): selectedness con dirtiness (E1).
    pub fn set_option_selected(&mut self, option: NodeId, value: bool) {
        if let ElementState::Option {
            selectedness,
            dirtiness,
            ..
        } = &mut self.get_mut(option).state
        {
            *selectedness = value;
            *dirtiness = true;
        }
    }

    fn set_option_selectedness(&mut self, option: NodeId, value: bool) {
        if let ElementState::Option { selectedness, .. } = &mut self.get_mut(option).state {
            *selectedness = value;
        }
    }

    fn cached_select(&self, option: NodeId) -> Option<NodeId> {
        match self.get(option).state {
            ElementState::Option { cached_select, .. } => cached_select,
            _ => None,
        }
    }

    fn selectedcontent_disabled(&self, id: NodeId) -> bool {
        matches!(
            self.get(id).state,
            ElementState::SelectedContent { disabled: true }
        )
    }

    /// Get the nearest ancestor select.
    fn nearest_ancestor_select(&self, element: NodeId) -> Option<NodeId> {
        let mut ancestor_optgroup = false;
        let mut current = self.parent(element);
        while let Some(ancestor) = current {
            if ["datalist", "hr", "option"]
                .iter()
                .any(|n| self.is_html_element(ancestor, n))
            {
                return None;
            }
            if self.is_html_element(ancestor, "optgroup") {
                if ancestor_optgroup {
                    return None;
                }
                ancestor_optgroup = true;
            }
            if self.is_html_element(ancestor, "select") {
                return Some(ancestor);
            }
            current = self.parent(ancestor);
        }
        None
    }

    /// Update an option's nearest ancestor select. Devuelve los selects cuyos
    /// `selectedcontent` hay que actualizar.
    fn update_option_nearest_select(&mut self, option: NodeId) -> Vec<NodeId> {
        let old_select = self.cached_select(option);
        let new_select = self.nearest_ancestor_select(option);
        if let ElementState::Option { cached_select, .. } = &mut self.get_mut(option).state {
            *cached_select = new_select;
        }
        if old_select == new_select {
            return Vec::new();
        }
        let mut result = Vec::new();
        for select in [old_select, new_select].into_iter().flatten() {
            let has_changed = self.selectedness_setting(select);
            if self.option_selectedness(option) || has_changed {
                result.push(select);
            }
        }
        result
    }

    /// Get the list of options.
    pub fn list_of_options(&self, select: NodeId) -> Vec<NodeId> {
        let mut options = Vec::new();
        let mut node = self.get(select).first_child;
        while let Some(n) = node {
            if self.is_html_element(n, "option") {
                options.push(n);
            }
            let skip_descendants = ["select", "hr", "option", "datalist"]
                .iter()
                .any(|name| self.is_html_element(n, name))
                || (self.is_html_element(n, "optgroup") && self.has_optgroup_between(n, select));
            node = self.next_in_subtree(n, select, skip_descendants);
        }
        options
    }

    /// ¿Hay un `optgroup` ancestro de `node` antes de llegar a `select`?
    fn has_optgroup_between(&self, node: NodeId, select: NodeId) -> bool {
        let mut current = self.parent(node);
        while let Some(n) = current {
            if n == select {
                return false;
            }
            if self.is_html_element(n, "optgroup") {
                return true;
            }
            current = self.parent(n);
        }
        false
    }

    /// Siguiente descendiente de `root` en tree order después de `node`.
    fn next_in_subtree(&self, node: NodeId, root: NodeId, skip_children: bool) -> Option<NodeId> {
        if !skip_children && let Some(child) = self.get(node).first_child {
            return Some(child);
        }
        let mut current = node;
        while current != root {
            if let Some(next) = self.get(current).next_sibling {
                return Some(next);
            }
            current = self.parent(current)?;
        }
        None
    }

    /// An option element is disabled.
    pub fn option_disabled(&self, option: NodeId) -> bool {
        if self.has_attr(option, "disabled") {
            return true;
        }
        let mut current = self.parent(option);
        while let Some(ancestor) = current {
            if ["select", "hr", "datalist", "option"]
                .iter()
                .any(|n| self.is_html_element(ancestor, n))
            {
                return false;
            }
            if self.is_html_element(ancestor, "optgroup") {
                return self.has_attr(ancestor, "disabled");
            }
            current = self.parent(ancestor);
        }
        false
    }

    /// Display size de un `select`.
    pub fn display_size(&self, select: NodeId) -> u64 {
        let size = self
            .element(select)
            .and_then(|e| e.attr("size"))
            .and_then(parse_non_negative_integer);
        size.unwrap_or(if self.has_attr(select, "multiple") {
            4
        } else {
            1
        })
    }

    /// The selectedness setting algorithm.
    fn selectedness_setting(&mut self, select: NodeId) -> bool {
        if self.has_attr(select, "multiple") {
            return false;
        }
        let mut has_changed = false;
        let mut first_enabled = None;
        let mut last_selected: Option<NodeId> = None;
        for option in self.list_of_options(select) {
            if self.option_selectedness(option) {
                if let Some(last) = last_selected {
                    self.set_option_selectedness(last, false);
                    has_changed = true;
                }
                last_selected = Some(option);
            }
            if first_enabled.is_none() && !self.option_disabled(option) {
                first_enabled = Some(option);
            }
        }
        if last_selected.is_none()
            && let Some(first) = first_enabled
            && self.display_size(select) == 1
        {
            self.set_option_selectedness(first, true);
            has_changed = true;
        }
        has_changed
    }

    /// Update descendant selectedcontent elements for an option.
    fn update_selectedcontents_for_option(&mut self, option: NodeId) {
        let Some(select) = self.cached_select(option) else {
            return;
        };
        if !self.option_selectedness(option) {
            return;
        }
        self.update_select_selectedcontents(select);
    }

    /// Update a select's descendant selectedcontent elements.
    fn update_select_selectedcontents(&mut self, select: NodeId) {
        if self.has_attr(select, "multiple") {
            return;
        }
        let targets: Vec<NodeId> = self
            .inclusive_descendants(select)
            .into_iter()
            .skip(1)
            .filter(|&n| {
                matches!(
                    self.get(n).state,
                    ElementState::SelectedContent { disabled: false }
                )
            })
            .collect();
        for selectedcontent in targets {
            self.update_selectedcontent(select, selectedcontent);
        }
    }

    /// Update a selectedcontent.
    fn update_selectedcontent(&mut self, select: NodeId, selectedcontent: NodeId) {
        if self.selectedcontent_disabled(selectedcontent) {
            return;
        }
        let option = self
            .list_of_options(select)
            .into_iter()
            .find(|&o| self.option_selectedness(o));
        // Replace all: quitar los hijos actuales e insertar los clones (o nada, para "clear").
        let old: Vec<_> = self.children(selectedcontent).collect();
        for child in old {
            self.remove(child);
        }
        if let Some(option) = option {
            let children: Vec<_> = self.children(option).collect();
            for child in children {
                let copy = self.clone_node(child, true);
                self.append(selectedcontent, copy);
            }
        }
    }

    /// Recalculate a selectedcontent element's disabledness.
    fn recalculate_selectedcontent_disabledness(
        &mut self,
        selectedcontent: NodeId,
    ) -> Option<NodeId> {
        let mut nearest_select = None;
        let mut disabled = false;
        let mut current = self.parent(selectedcontent);
        while let Some(ancestor) = current {
            if self.is_html_element(ancestor, "select") {
                if nearest_select.is_none() {
                    nearest_select = Some(ancestor);
                    current = self.parent(ancestor);
                    continue;
                }
                disabled = true;
                break;
            }
            if self.is_html_element(ancestor, "option")
                || self.is_html_element(ancestor, "selectedcontent")
            {
                disabled = true;
                break;
            }
            current = self.parent(ancestor);
        }
        self.get_mut(selectedcontent).state = ElementState::SelectedContent { disabled };
        nearest_select
    }
}

impl Document {
    /// Label de un `option`: el atributo `label` si no está vacío o, si no, su texto con el
    /// white space ASCII quitado de los extremos y colapsado (4.10.10).
    pub fn option_label(&self, option: NodeId) -> String {
        if let Some(label) = self
            .element(option)
            .and_then(|e| e.attr("label"))
            .filter(|l| !l.is_empty())
        {
            return label.to_owned();
        }
        self.text_content(option)
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Concatenación de los nodos de texto descendientes (DOM `textContent` de un elemento).
    pub fn text_content(&self, node: NodeId) -> String {
        let mut out = String::new();
        for n in self.inclusive_descendants(node) {
            if let crate::NodeData::Text(t) | crate::NodeData::CdataSection(t) = &self.get(n).data {
                out.push_str(&t.to_string_lossy());
            }
        }
        out
    }
}

/// Rules for parsing non-negative integers (2.3.4.1).
fn parse_non_negative_integer(input: &str) -> Option<u64> {
    let s = input.trim_start_matches(['\t', '\n', '\x0C', '\r', ' ']);
    let s = s.strip_prefix('+').unwrap_or(s);
    let digits: &str = &s[..s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len())];
    if digits.is_empty() {
        return None;
    }
    Some(digits.parse::<u64>().unwrap_or(u64::MAX))
}
