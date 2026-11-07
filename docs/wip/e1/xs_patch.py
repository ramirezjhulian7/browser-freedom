import sys
sys.path.insert(0, "D:/Personales/Repos/browser-freedom-army-e1/docs/wip/e1")
from patchlib import patch
patch("crates/bf-web/src/dom/serializer.rs", [
("""use crate::dom::{classes, from_dom_string, unwrap_node};""", """use crate::dom::{from_dom_string, unwrap_node};"""),
("""/// El payload de un `XMLSerializer` (sin estado).
#[derive(Debug, Clone, Copy)]
pub struct XmlSerializerData;
""", """/// El payload de un `XMLSerializer` (sin estado).
#[derive(Debug, Clone, Copy)]
pub struct XmlSerializerData;

/// La clase registrada (una vez por agente).
#[derive(Debug, Clone, Copy)]
struct XmlSerializerClass(bf_js::host::HostClassId);

/// Registra `XMLSerializer`.
pub fn register(agent: &mut Agent) {
    if agent.embedder_data::<XmlSerializerClass>().is_some() {
        return;
    }
    let class = agent.register_host_class(xml_serializer_class());
    agent.set_embedder_data(XmlSerializerClass(class));
}
"""),
("""    let class = classes(agent).xml_serializer;""", """    let class = agent.embedder_data::<XmlSerializerClass>().expect("XMLSerializer").0;"""),
])
patch("crates/bf-web/src/dom/mod.rs", [
("""pub mod range;
pub mod shadow; // E1""", """pub mod range;
pub mod serializer; // E1
pub mod shadow; // E1"""),
])
patch("crates/bf-web/src/html/mod.rs", [
("""    crate::dom::shadow::register(agent);""", """    crate::dom::shadow::register(agent);
    crate::dom::serializer::register(agent); // E1"""),
])
print("ok")
