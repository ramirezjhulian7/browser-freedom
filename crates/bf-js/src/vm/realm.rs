//! Realms (§9.3): los intrínsecos, el objeto global, el Global Environment y el registro de
//! templates de cada realm.

use std::collections::{HashMap, HashSet};

use crate::heap::{Gc, Trace, Tracer};
use crate::object::Object;
use crate::vm::env::{Env, EnvKind, GlobalEnv};
use crate::vm::{Agent, RealmId};

macro_rules! intrinsics {
    ($($field:ident),* $(,)?) => {
        /// Los objetos intrínsecos (§6.1.7.4) de un realm.
        #[derive(Debug, Clone)]
        pub struct Intrinsics {
            $(pub $field: Gc<Object>,)*
        }

        impl Intrinsics {
            /// Todos los campos apuntando a `o` (se completan al crear el realm).
            pub fn placeholder(o: Gc<Object>) -> Intrinsics {
                Intrinsics { $($field: o,)* }
            }
        }

        impl Trace for Intrinsics {
            fn trace(&self, t: &mut Tracer) {
                $(t.gc(self.$field);)*
            }
        }
    };
}

intrinsics!(
    object_prototype,
    function_prototype,
    object_ctor,
    function_ctor,
    array_ctor,
    array_prototype,
    array_proto_values,
    array_iterator_prototype,
    string_ctor,
    string_prototype,
    string_iterator_prototype,
    number_ctor,
    number_prototype,
    boolean_ctor,
    boolean_prototype,
    symbol_ctor,
    symbol_prototype,
    bigint_prototype,
    error_ctor,
    error_prototype,
    eval_error_ctor,
    eval_error_prototype,
    range_error_ctor,
    range_error_prototype,
    reference_error_ctor,
    reference_error_prototype,
    syntax_error_ctor,
    syntax_error_prototype,
    type_error_ctor,
    type_error_prototype,
    uri_error_ctor,
    uri_error_prototype,
    aggregate_error_ctor,
    aggregate_error_prototype,
    iterator_prototype,
    generator_function,
    generator_function_prototype,
    generator_prototype,
    throw_type_error,
    eval,
    reflect,
    math,
    for_in_iterator_prototype,
    // E2
    regexp_ctor,
    regexp_prototype,
    regexp_string_iterator_prototype,
    bigint_ctor,
    date_ctor,
    date_prototype,
);

pub struct Realm {
    pub intrinsics: Intrinsics,
    pub global_object: Gc<Object>,
    pub global_env: Gc<Env>,
    /// `[[TemplateMap]]`: sitio → objeto template.
    pub template_map: HashMap<u64, Gc<Object>>,
}

impl Trace for Realm {
    fn trace(&self, t: &mut Tracer) {
        self.intrinsics.trace(t);
        t.gc(self.global_object);
        t.gc(self.global_env);
        for o in self.template_map.values() {
            t.gc(*o);
        }
    }
}

impl Agent {
    /// CreateRealm (§9.3.1) + SetRealmGlobalObject + SetDefaultGlobalBindings: un realm nuevo
    /// con sus intrínsecos y su objeto global.
    pub fn create_realm(&mut self) -> RealmId {
        let object_prototype = self
            .heap
            .alloc(Object::new(None, crate::object::ObjectKind::Ordinary));
        let id = self.realms.len();
        let global_env = self.heap.alloc(Env {
            outer: None,
            kind: EnvKind::Global(Box::new(GlobalEnv {
                object: object_prototype,
                this: object_prototype,
                decl: HashMap::new(),
                var_names: HashSet::new(),
            })),
        });
        self.realms.push(Realm {
            intrinsics: Intrinsics::placeholder(object_prototype),
            global_object: object_prototype,
            global_env,
            template_map: HashMap::new(),
        });
        let saved = self.current_realm;
        self.current_realm = id;
        crate::builtins::init_realm(self, id, object_prototype);
        // El objeto global (OrdinaryObjectCreate(%Object.prototype%)) y sus propiedades.
        let global = self.new_object(Some(object_prototype));
        self.realms[id].global_object = global;
        if let Some(g) = self.heap[global_env].global_mut() {
            g.object = global;
            g.this = global;
        }
        crate::builtins::global::init_global_object(self, global);
        self.current_realm = saved;
        id
    }
}
