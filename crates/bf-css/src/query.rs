//! Selectores desde el DOM (DOM §4.2.6 "scope-match a selectors string" y §4.9
//! `matches`/`closest`): viven aquí porque `bf-css` depende de `bf-dom`.

use bf_dom::{Document, DomException, NodeId};

use crate::selectors::{MatchContext, NamespaceMap, SelectorList, matches_list, parse_selector_list};

/// "Parse a selector": `SyntaxError` si es inválido.
fn parse(selectors: &str) -> Result<SelectorList, DomException> {
    parse_selector_list(selectors, &NamespaceMap::default()).map_err(|_| DomException::SyntaxError)
}

fn context(doc: &Document, scope: NodeId) -> MatchContext {
    MatchContext {
        scope: Some(scope),
        ..MatchContext::for_document(doc)
    }
}

/// `Element.matches(selectors)` / `webkitMatchesSelector`: con `:scope` = el elemento.
pub fn element_matches(doc: &Document, el: NodeId, selectors: &str) -> Result<bool, DomException> {
    let list = parse(selectors)?;
    Ok(doc.element(el).is_some() && matches_list(&list, doc, el, &context(doc, el)))
}

/// `Element.closest(selectors)`: el primer ancestro inclusivo que coincide.
pub fn closest(doc: &Document, el: NodeId, selectors: &str) -> Result<Option<NodeId>, DomException> {
    let list = parse(selectors)?;
    let ctx = context(doc, el);
    Ok(doc
        .inclusive_ancestors(el)
        .into_iter()
        .find(|&n| doc.element(n).is_some() && matches_list(&list, doc, n, &ctx)))
}

/// "Scope-match a selectors string" contra `node`: los descendientes elemento de `node` que
/// coinciden, en tree order, con `:scope` = `node`.
pub fn query_selector_all(
    doc: &Document,
    node: NodeId,
    selectors: &str,
) -> Result<Vec<NodeId>, DomException> {
    let list = parse(selectors)?;
    let ctx = context(doc, node);
    Ok(doc
        .descendants(node)
        .into_iter()
        .filter(|&n| doc.element(n).is_some() && matches_list(&list, doc, n, &ctx))
        .collect())
}

/// `querySelector(selectors)`: el primero de [`query_selector_all`].
pub fn query_selector(
    doc: &Document,
    node: NodeId,
    selectors: &str,
) -> Result<Option<NodeId>, DomException> {
    let list = parse(selectors)?;
    let ctx = context(doc, node);
    let mut cur = doc.following_within(node, node);
    while let Some(n) = cur {
        if doc.element(n).is_some() && matches_list(&list, doc, n, &ctx) {
            return Ok(Some(n));
        }
        cur = doc.following_within(n, node);
    }
    Ok(None)
}
