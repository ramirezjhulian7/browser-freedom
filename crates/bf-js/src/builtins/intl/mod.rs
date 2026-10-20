//! Intl (ECMA-402).

use crate::heap::{Trace, Tracer};
use crate::vm::{Agent, RealmId};

/// Los slots de los objetos de Intl.
#[derive(Debug)]
pub enum IntlData {}

impl Trace for IntlData {
    fn trace(&self, _: &mut Tracer) {}
}

pub fn init(_: &mut Agent, _: RealmId) {}
