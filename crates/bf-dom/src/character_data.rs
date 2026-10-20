//! Interfaces `CharacterData` (DOM §4.10) y `Text` (§4.11). Los offsets son en code units
//! UTF-16.

use bf_infra::DomString;

use crate::observer::{Mutation, MutationType};
use crate::{Document, DomException, NodeData, NodeId};

impl Document {
    fn char_data(&self, n: NodeId) -> &DomString {
        match &self.get(n).data {
            NodeData::Text(d)
            | NodeData::CdataSection(d)
            | NodeData::Comment(d)
            | NodeData::ProcessingInstruction { data: d, .. } => d,
            _ => panic!("no es CharacterData"),
        }
    }

    fn char_data_mut(&mut self, n: NodeId) -> &mut DomString {
        match &mut self.get_mut(n).data {
            NodeData::Text(d)
            | NodeData::CdataSection(d)
            | NodeData::Comment(d)
            | NodeData::ProcessingInstruction { data: d, .. } => d,
            _ => panic!("no es CharacterData"),
        }
    }

    /// `CharacterData.data`.
    pub fn data(&self, n: NodeId) -> DomString {
        self.char_data(n).clone()
    }

    /// Setter de `CharacterData.data`: *replace data* de todo el contenido.
    pub fn set_data(&mut self, n: NodeId, value: &DomString) {
        let len = self.node_length(n);
        let _ = self.replace_data(n, 0, len, value);
    }

    /// `CharacterData.length`.
    pub fn data_length(&self, n: NodeId) -> usize {
        self.char_data(n).len_utf16()
    }

    /// "Substring data".
    pub fn substring_data(
        &self,
        n: NodeId,
        offset: usize,
        count: usize,
    ) -> Result<DomString, DomException> {
        let length = self.data_length(n);
        if offset > length {
            return Err(DomException::IndexSizeError);
        }
        let end = offset.saturating_add(count).min(length);
        Ok(self.char_data(n).substring_utf16(offset, end))
    }

    /// "Replace data" (§4.10): reemplaza `count` code units desde `offset` por `data`.
    pub fn replace_data(
        &mut self,
        n: NodeId,
        offset: usize,
        count: usize,
        data: &DomString,
    ) -> Result<(), DomException> {
        // 1-3.
        let length = self.data_length(n);
        if offset > length {
            return Err(DomException::IndexSizeError);
        }
        let count = count.min(length - offset);
        // 4.
        let old = self.char_data(n).clone();
        self.queue_mutation_record(Mutation {
            type_: MutationType::CharacterData,
            target: n,
            name: None,
            namespace: None,
            old_value: Some(&old),
            added: &[],
            removed: &[],
            previous: None,
            next: None,
        });
        // 5-6.
        let units = old.to_utf16();
        let mut new_units = units[..offset].to_vec();
        new_units.extend(data.code_units());
        new_units.extend_from_slice(&units[offset + count..]);
        *self.char_data_mut(n) = DomString::from_utf16(&new_units);
        self.bump_version();
        Ok(())
    }

    /// `CharacterData.appendData(data)`.
    pub fn append_data(&mut self, n: NodeId, data: &DomString) {
        let len = self.data_length(n);
        let _ = self.replace_data(n, len, 0, data);
    }

    /// `CharacterData.insertData(offset, data)`.
    pub fn insert_data(
        &mut self,
        n: NodeId,
        offset: usize,
        data: &DomString,
    ) -> Result<(), DomException> {
        self.replace_data(n, offset, 0, data)
    }

    /// `CharacterData.deleteData(offset, count)`.
    pub fn delete_data(
        &mut self,
        n: NodeId,
        offset: usize,
        count: usize,
    ) -> Result<(), DomException> {
        self.replace_data(n, offset, count, &DomString::new())
    }

    /// `Text.splitText(offset)` ("split a Text node").
    pub fn split_text(&mut self, n: NodeId, offset: usize) -> Result<NodeId, DomException> {
        let length = self.data_length(n);
        if offset > length {
            return Err(DomException::IndexSizeError);
        }
        let count = length - offset;
        let new_data = self.substring_data(n, offset, count)?;
        let new_node = match &self.get(n).data {
            NodeData::CdataSection(_) => self.create(NodeData::CdataSection(new_data)),
            _ => self.create(NodeData::Text(new_data)),
        };
        if let Some(parent) = self.parent(n) {
            let next = self.get(n).next_sibling();
            self.insert(new_node, parent, next, false);
        }
        self.replace_data(n, offset, count, &DomString::new())?;
        Ok(new_node)
    }

    /// `Text.wholeText`: los datos de los `Text` exclusivos contiguos a `n`.
    pub fn whole_text(&self, n: NodeId) -> DomString {
        let mut first = n;
        while let Some(p) = self.get(first).prev_sibling()
            && self.is_exclusive_text(p)
        {
            first = p;
        }
        let mut out = DomString::new();
        let mut cur = Some(first);
        while let Some(c) = cur {
            if !self.is_exclusive_text(c) && c != n {
                break;
            }
            out.push_dom_str(self.char_data(c));
            cur = self.get(c).next_sibling();
        }
        out
    }
}
