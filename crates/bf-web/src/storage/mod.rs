//! Almacenamiento del lado del cliente: IndexedDB 3.0 ([`idb`]) sobre un almacén propio
//! ([`backend`]) y `navigator.storage` (`StorageManager`, Storage Standard §4). **Dueño: E2.**
//!
//! Los datos van a `<storage_dir>/<origen>/` ([`crate::WebHooks::storage_dir`], que el shell
//! apunta a `%APPDATA%\freedom\idb\`); sin directorio, quedan en memoria del agente.

pub mod backend;
pub mod idb;

use std::path::PathBuf;

use bf_js::vm::{Agent, RealmId};

use crate::WebContextHandle;
use backend::Backend;

/// La cuota que se anuncia (`estimate().quota`): 10 GiB.
pub fn quota() -> f64 {
    10.0 * 1024.0 * 1024.0 * 1024.0
}

/// El estado del almacenamiento de una ventana (dato del realm).
struct StorageState {
    backend: Backend,
    /// El directorio raíz elegido por el embebedor o los tests.
    root: Option<PathBuf>,
}

/// Fija el directorio raíz del almacenamiento de `realm` (los datos van a
/// `<dir>/<origen>/`); para los tests y los embebedores sin `WebHooks::storage_dir`.
pub fn set_storage_dir(agent: &mut Agent, realm: RealmId, dir: Option<PathBuf>) {
    agent.set_realm_data(
        realm,
        StorageState {
            backend: Backend::new(None),
            root: dir,
        },
    );
    refresh_dir(agent, realm);
}

/// El directorio del origen: `<root>/<origen>`.
fn refresh_dir(agent: &mut Agent, realm: RealmId) {
    let origin = {
        let ctx = WebContextHandle { realm }.get(agent);
        ctx.url.origin().to_string()
    };
    let hooks_dir = WebContextHandle { realm }.get(agent).hooks.storage_dir();
    if let Some(s) = agent.realm_data_mut::<StorageState>(realm) {
        let root = s.root.clone().or(hooks_dir);
        s.backend.dir = root.map(|r| r.join(backend::origin_dir_name(&origin)));
    }
}

/// El almacén del realm en curso.
fn backend_mut(agent: &mut Agent) -> &mut Backend {
    let realm = WebContextHandle::current(agent).realm;
    if agent.realm_data::<StorageState>(realm).is_none() {
        agent.set_realm_data(
            realm,
            StorageState {
                backend: Backend::new(None),
                root: None,
            },
        );
        refresh_dir(agent, realm);
    }
    &mut agent
        .realm_data_mut::<StorageState>(realm)
        .expect("StorageState")
        .backend
}

/// Registra IndexedDB y `navigator.storage` (lo llama [`crate::register_all`]).
pub fn register(agent: &mut Agent, realm: RealmId, _ctx: &WebContextHandle) {
    idb::install(agent, realm);
}
