//! Colecciones (DOM §4.2.10): `HTMLCollection` viva y `NodeList` de `childNodes`.
//!
//! Una [`HtmlCollection`] guarda su raíz y su filtro; sus elementos se recalculan solo cuando
//! cambia el contador de mutaciones del documento ([`Document::version`]).

use std::cell::RefCell;

use bf_infra::{DomString, Namespace};

use crate::element::ordered_set;
use crate::{Document, NodeId, QuirksMode};

/// Qué elementos contiene una colección.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionFilter {
    /// Los hijos elemento de la raíz (`children`).
    Children,
    /// "List of elements with qualified name" (`getElementsByTagName`).
    QualifiedName(DomString),
    /// "List of elements with namespace and local name" (`None` = `*`).
    NamespaceLocal {
        namespace: Option<Option<Namespace>>,
        local: Option<DomString>,
    },
    /// "List of elements with class names".
    ClassNames(Vec<DomString>),
}

/// `HTMLCollection` viva.
#[derive(Debug, Clone)]
pub struct HtmlCollection {
    pub root: NodeId,
    pub filter: CollectionFilter,
    cache: RefCell<Option<(u64, Vec<NodeId>)>>,
}

impl HtmlCollection {
    pub fn new(root: NodeId, filter: CollectionFilter) -> Self {
        HtmlCollection {
            root,
            filter,
            cache: RefCell::new(None),
        }
    }

    fn matches(&self, doc: &Document, n: NodeId) -> bool {
        let Some(e) = doc.element(n) else {
            return false;
        };
        match &self.filter {
            CollectionFilter::Children => doc.parent(n) == Some(self.root),
            CollectionFilter::QualifiedName(q) => {
                if *q == "*" {
                    return true;
                }
                let qn = e.qualified_name();
                if doc.is_html && e.ns == Namespace::Html {
                    qn == q.ascii_lowercase()
                } else {
                    qn == *q
                }
            }
            CollectionFilter::NamespaceLocal { namespace, local } => {
                let ns_ok = match namespace {
                    None => true,
                    Some(None) => e.ns == Namespace::Null,
                    Some(Some(ns)) => e.ns == *ns,
                };
                ns_ok && local.as_ref().is_none_or(|l| e.name == *l)
            }
            CollectionFilter::ClassNames(classes) => {
                if classes.is_empty() {
                    return false;
                }
                let own = e
                    .attrs
                    .iter()
                    .find(|a| a.ns.is_none() && a.name == "class")
                    .map(|a| ordered_set(&a.value))
                    .unwrap_or_default();
                let quirks = doc.mode == QuirksMode::Quirks;
                classes.iter().all(|c| {
                    own.iter().any(|o| {
                        if quirks {
                            o.eq_ignore_ascii_case(c)
                        } else {
                            o == c
                        }
                    })
                })
            }
        }
    }

    /// Los elementos en tree order.
    pub fn items(&self, doc: &Document) -> Vec<NodeId> {
        if let Some((v, items)) = &*self.cache.borrow()
            && *v == doc.version()
        {
            return items.clone();
        }
        let items: Vec<NodeId> = match self.filter {
            CollectionFilter::Children => doc.element_children(self.root),
            _ => doc
                .descendants(self.root)
                .into_iter()
                .filter(|&n| self.matches(doc, n))
                .collect(),
        };
        *self.cache.borrow_mut() = Some((doc.version(), items.clone()));
        items
    }

    pub fn length(&self, doc: &Document) -> usize {
        self.items(doc).len()
    }

    pub fn item(&self, doc: &Document, index: usize) -> Option<NodeId> {
        self.items(doc).get(index).copied()
    }

    /// `namedItem(key)`: el primero con ese `id` o, si es HTML, con ese `name`.
    pub fn named_item(&self, doc: &Document, key: &DomString) -> Option<NodeId> {
        if key.is_empty() {
            return None;
        }
        self.items(doc).into_iter().find(|&n| {
            let Some(e) = doc.element(n) else {
                return false;
            };
            e.attrs.iter().any(|a| {
                a.ns.is_none()
                    && ((a.name == "id" && a.value == *key)
                        || (e.ns == Namespace::Html && a.name == "name" && a.value == *key))
            })
        })
    }
}

impl Document {
    /// `ParentNode.children`.
    pub fn children_collection(&self, n: NodeId) -> HtmlCollection {
        HtmlCollection::new(n, CollectionFilter::Children)
    }

    /// `getElementsByTagName(qualifiedName)`.
    pub fn get_elements_by_tag_name(&self, root: NodeId, qualified_name: &DomString) -> HtmlCollection {
        HtmlCollection::new(root, CollectionFilter::QualifiedName(qualified_name.clone()))
    }

    /// `getElementsByTagNameNS(namespace, localName)` (`"*"` es comodín en ambos).
    pub fn get_elements_by_tag_name_ns(
        &self,
        root: NodeId,
        namespace: Option<&str>,
        local: &DomString,
    ) -> HtmlCollection {
        let namespace = match namespace {
            Some("*") => None,
            Some(uri) => Some(Namespace::from_uri(uri)),
            None => Some(None),
        };
        let local = (*local != "*").then(|| local.clone());
        HtmlCollection::new(root, CollectionFilter::NamespaceLocal { namespace, local })
    }

    /// `getElementsByClassName(classNames)`.
    pub fn get_elements_by_class_name(&self, root: NodeId, class_names: &DomString) -> HtmlCollection {
        HtmlCollection::new(root, CollectionFilter::ClassNames(ordered_set(class_names)))
    }
}
