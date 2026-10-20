//! El bytecode: una máquina de pila. Cada función compilada es un [`FunctionCode`]
//! (instrucciones, constantes, cantidad de slots locales, tabla de posiciones y lo que la VM
//! necesita para crear y llamar la función). El código no tiene valores del heap: se puede
//! compartir entre realms y cachear.

use std::cell::Cell;
use std::rc::Rc;

use crate::ast::Span;
use crate::string::{Atom, JsString};
use crate::value::PropertyKey;
use crate::vm::env::ScopeLayout;

/// El texto fuente de un script, `eval` o función dinámica.
#[derive(Debug)]
pub struct Source {
    pub name: String,
    pub units: Vec<u16>,
    /// Offset (UTF-16) del comienzo de cada línea.
    pub line_starts: Vec<u32>,
}

impl Source {
    pub fn new(name: &str, text: &str) -> Source {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut line_starts = vec![0];
        let mut i = 0;
        while i < units.len() {
            let u = units[i];
            let nl = match u {
                0x0D => {
                    if units.get(i + 1) == Some(&0x0A) {
                        i += 1;
                    }
                    true
                }
                0x0A | 0x2028 | 0x2029 => true,
                _ => false,
            };
            i += 1;
            if nl {
                line_starts.push(i as u32);
            }
        }
        Source {
            name: name.to_owned(),
            units,
            line_starts,
        }
    }

    /// (línea, columna), ambas desde 1.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line as u32 + 1, offset - self.line_starts[line] + 1)
    }

    pub fn slice(&self, span: Span) -> JsString {
        let s = (span.start as usize).min(self.units.len());
        let e = (span.end as usize).clamp(s, self.units.len());
        JsString::from_units(self.units[s..e].to_vec())
    }
}

/// Qué clase de código es (determina `[[ThisMode]]`, `[[ConstructorKind]]`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Script,
    Eval,
    /// Declaración o expresión `function`.
    Normal,
    Arrow,
    /// Método de objeto o de clase (incluidos getters y setters).
    Method,
    /// El constructor de una clase (`derived` si tiene `extends`).
    ClassConstructor {
        derived: bool,
    },
    /// El inicializador de un campo de clase (§15.7.10).
    FieldInit,
    /// Un bloque `static {}` (§15.7.11).
    StaticBlock,
}

/// Una constante del pool.
#[derive(Debug, Clone)]
pub enum Constant {
    Number(f64),
    String(JsString),
    /// Un nombre de binding o de propiedad.
    Name(Atom, PropertyKey),
    BigInt(String),
    Function(Rc<FunctionCode>),
    Layout(Rc<ScopeLayout>),
    /// Un sitio de template (§13.2.8.4): strings cooked (`None` si inválido) y raw.
    Template(Rc<TemplateSite>),
    RegExp {
        pattern: JsString,
        flags: JsString,
    },
    /// Tabla de continuaciones de un `finally`: (clase de completion, destino).
    FinallyTable(Rc<Vec<(u32, u32)>>),
    /// Mapeo de los parámetros a slots del environment (arguments mapeado).
    ParamMap(Rc<Vec<Option<u32>>>),
}

#[derive(Debug)]
pub struct TemplateSite {
    pub id: u64,
    pub cooked: Vec<Option<JsString>>,
    pub raw: Vec<JsString>,
}

thread_local! {
    static NEXT_SITE: Cell<u64> = const { Cell::new(1) };
}

/// Un identificador único por sitio de template (por Parse Node).
pub fn next_template_site() -> u64 {
    NEXT_SITE.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

/// De dónde sale el nombre de una función al crearla (SetFunctionName).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnName {
    /// El nombre del código (`""` si es anónima).
    Code,
    /// La clave de propiedad que está en el tope de la pila (con prefijo).
    Stack(NamePrefix),
    /// La clave guardada en un slot local.
    Local(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamePrefix {
    None,
    Get,
    Set,
}

/// Dónde está el contexto de la función (`this`, `new.target`, la función activa) que
/// usa una referencia: en el frame propio, en el Function Environment `hops` niveles hacia
/// afuera (desde flechas), o buscándolo dinámicamente (código de `eval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxLoc {
    Frame,
    Env(u32),
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxWhat {
    This,
    NewTarget,
    /// La función activa (para `super()` y `[[HomeObject]]`).
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    TypeError,
    ReferenceError,
    SyntaxError,
    RangeError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Method,
    Getter,
    Setter,
}

/// Clases de completion para `finally` (en el slot de clase). Las mayores que
/// `COMPLETION_RETURN` son saltos (`break`/`continue`) registrados en la tabla del `finally`.
pub const COMPLETION_NORMAL: u32 = 0;
pub const COMPLETION_THROW: u32 = 1;
pub const COMPLETION_RETURN: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    // --- Pila ---
    Pop,
    Dup,
    /// a b → a b a b
    Dup2,
    /// a b c → a b c a b c
    Dup3,
    Swap,
    /// a b → a b a
    Over,
    /// Mueve el elemento a profundidad n al tope (Pull(1) = Swap).
    Pull(u8),
    /// Mueve el tope a profundidad n (a b c, Bury(2) → c a b).
    Bury(u8),
    Undefined,
    Null,
    True,
    False,
    Int(i32),
    Const(u32),
    BigInt(u32),
    RegExp(u32),

    // --- Bindings ---
    GetLocal(u32),
    GetLocalChecked(u32, u32),
    /// Asigna sin sacar el valor.
    SetLocal(u32),
    SetLocalChecked(u32, u32),
    /// Inicializa sacando el valor.
    InitLocal(u32),
    /// Deja el slot sin inicializar (TDZ).
    ClearLocal(u32),
    /// Asignación a un `const`: ReferenceError si está en TDZ, si no TypeError.
    ConstAssignLocal(u32, u32),
    GetEnv(u32, u32),
    GetEnvChecked(u32, u32, u32),
    SetEnv(u32, u32),
    SetEnvChecked(u32, u32, u32),
    InitEnv(u32, u32),
    ConstAssignEnv(u32, u32, u32),
    /// Búsqueda dinámica (§9.1.2.1) desde el environment actual; ReferenceError si no está.
    GetName(u32),
    /// Igual que `GetName` pero empezando en el Global Environment.
    GetGlobal(u32),
    /// `typeof nombre` (no lanza si no está).
    TypeofName(u32),
    TypeofGlobal(u32),
    /// ResolveBinding: guarda la referencia en la pila de referencias del frame.
    ResolveRef(u32),
    ResolveGlobalRef(u32),
    /// GetValue de la referencia del tope (sin sacarla).
    GetRefValue(u32),
    /// PutValue en la referencia del tope (la saca); el valor queda en la pila. El bool es
    /// la estrictez del código.
    PutRef(u32, bool),
    /// Descarta la referencia del tope.
    PopRef,
    /// Inicializa (InitializeReferencedBinding) la referencia del tope; saca valor y ref.
    InitRef(u32),
    DeleteName(u32),
    /// Empuja la función y su `this` (el objeto del `with`, o `undefined`).
    GetNameForCall(u32),
    GetGlobalForCall(u32),
    /// La función que se está ejecutando (el binding del nombre de una expresión de
    /// función que ninguna clausura captura).
    GetCallee,
    PushScope(u32),
    /// El Function Environment de la función actual (con `this`, `new.target` y la función).
    PushFunctionScope(u32),
    /// El environment actual pasa a ser el VariableEnvironment (para `eval` no estricto).
    MarkVarEnv,
    PopScope,
    /// CreatePerIterationEnvironment (§14.7.4.4): reemplaza el environment actual por una
    /// copia.
    CopyScope,
    /// `with`: ToObject del tope y Object Environment.
    PushWith,

    // --- Contexto de la función ---
    Ctx(CtxWhat, CtxLoc),
    /// BindThisValue (§9.1.1.3.1) del tope; ReferenceError si ya estaba inicializado. Hace
    /// también InitializeInstanceElements con la función activa (fin de `super(...)`).
    SuperCallFinish(CtxLoc),
    /// [función] → [función.[[GetPrototypeOf]]()] (GetSuperConstructor).
    SuperConstructor,
    /// [home] → [home.[[GetPrototypeOf]]()] (GetSuperBase); `home` puede ser undefined.
    SuperBase,
    /// [this key base] → valor.
    GetSuperElem,
    /// [this key base value] → value.
    SetSuperElem(bool),
    /// [función] → [función.[[HomeObject]]].
    HomeObject,

    // --- Operadores ---
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    Shl,
    Shr,
    UShr,
    BitAnd,
    BitOr,
    BitXor,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    In,
    InstanceOf,
    /// [privateName obj] → bool (`#x in obj`).
    PrivateIn,
    Neg,
    Plus,
    Not,
    BitNot,
    Typeof,
    ToNumeric,
    Inc,
    Dec,
    ToPropertyKey,
    /// ToPropertyKey del valor a profundidad n (en su lugar).
    ToPropertyKeyAt(u8),
    ToStringOp,
    /// TypeError si el valor a profundidad n es null/undefined (sin sacarlo).
    CheckCoercible(u8),
    /// [obj key] → [obj key'] con el chequeo de coercibilidad y ToPropertyKey.
    PrepareElem,

    // --- Propiedades ---
    GetProp(u32),
    GetElem,
    /// [obj] → [obj valor]
    GetPropKeep(u32),
    /// [obj key] → [obj valor]
    GetElemKeep,
    /// [obj valor] → valor
    SetProp(u32, bool),
    /// [obj key valor] → valor
    SetElem(bool),
    DeleteProp(u32, bool),
    DeleteElem(bool),
    /// [obj valor] → obj (CreateDataPropertyOrThrow).
    InitProp(u32),
    /// [obj key valor] → obj
    InitElem,
    /// [obj valor] → obj (`__proto__: valor` en un literal, B.3.1).
    InitProto,
    /// [obj key? fn] → obj: define un getter o setter en un literal (enumerable).
    InitAccessor {
        name: u32,
        setter: bool,
        computed: bool,
    },
    /// [obj fuente] → obj (spread en literales de objeto).
    CopyDataProperties,
    /// [fuente exclusiones... (n)] → objeto rest (CopyDataProperties con exclusiones).
    CopyRest(u32),
    /// [privateName obj] → valor
    PrivateGet,
    /// [privateName obj valor] → valor
    PrivateSet,
    NewObject,
    NewArray,
    /// [arr v] → arr
    ArrayPush,
    /// [arr] → arr con length+1 (un hueco).
    ArrayHole,
    /// [arr iterable] → arr
    ArraySpread,

    // --- Llamadas ---
    /// [f this args…] → resultado
    /// (argc, constante con el texto del callee para los mensajes; u32::MAX si no hay).
    Call(u32, u32),
    /// [f this arr] → resultado
    CallSpread(u32),
    /// Posible `eval` directo: [f this args…] (flags: los de PerformEval).
    CallEval(u32, u32),
    CallEvalSpread(u32),
    /// [ctor args…] → resultado
    New(u32, u32),
    NewSpread(u32),
    /// [newTarget ctor args…] → resultado (super(...)); TypeError si ctor no es constructor.
    SuperConstruct(u32),
    SuperConstructSpread,
    /// [newTarget ctor] → resultado con los argumentos del frame (constructor derivado por
    /// defecto, §15.7.14 paso 14.a).
    SuperConstructForward,
    /// `[[ClassFieldInitializerName]]` de la función activa, como clave.
    FieldName,
    /// [tag] → [tag templateObject]
    TemplateObject(u32),
    Closure {
        code: u32,
        name: FnName,
        /// Profundidad en la pila del HomeObject (0 = sin HomeObject).
        home: u8,
    },

    // --- Control ---
    Jump(u32),
    JumpIfTrue(u32),
    JumpIfFalse(u32),
    /// Salta sin sacar si es truthy / falsy; si no, saca.
    JumpIfTrueKeep(u32),
    JumpIfFalseKeep(u32),
    /// `??`: salta sin sacar si no es nullish; si no, saca.
    JumpIfNotNullishKeep(u32),
    /// Salta sin sacar si no es undefined (valores por defecto).
    JumpIfNotUndefined(u32),
    /// Cadena opcional: si el tope es nullish lo saca (con `extra` valores más), empuja
    /// undefined y salta.
    JumpIfNullishChain {
        target: u32,
        extra: u8,
    },
    Throw,
    ThrowError(ErrorKind, u32),
    /// Algo no implementado todavía: InternalError (cuenta como fallo).
    Unsupported(u32),
    /// Empuja un handler de excepciones. `finally` guarda (clase, valor) en dos slots.
    PushCatch(u32),
    PushFinally {
        target: u32,
        kind_slot: u32,
        value_slot: u32,
    },
    PopHandler,
    /// Fin de un `finally`: según la clase de completion guardada, sigue, relanza, retorna o
    /// salta a la continuación de la tabla.
    EndFinally {
        kind_slot: u32,
        value_slot: u32,
        table: u32,
    },
    Return,
    Debugger,

    // --- Iteración ---
    /// [obj] → (): GetIterator; guarda iterador, next y done en slot, slot+1, slot+2.
    GetIterator(u32),
    /// IteratorStepValue: empuja el valor o salta a `done` (marcando done).
    IterStep {
        slot: u32,
        done: u32,
    },
    /// Para destructuring: el siguiente valor, o undefined si ya terminó.
    IterValue(u32),
    /// [] → array con el resto.
    IterRest(u32),
    /// IteratorClose normal si no terminó.
    IterClose(u32),
    /// Handler de limpieza de un iterador (completion en kind/value): throw → cierra
    /// ignorando errores y relanza; return → cierra y sigue retornando.
    IterCleanup {
        slot: u32,
        kind_slot: u32,
        value_slot: u32,
    },
    /// [obj] → (): CreateForInIterator en el slot (sin iteraciones si es nullish).
    ForInPrepare(u32),
    ForInNext {
        slot: u32,
        done: u32,
    },

    // --- Generadores ---
    /// Fin del prólogo: crea el generador y suspende el frame.
    GeneratorStart,
    Yield,
    /// `yield*`: un paso de la delegación (§14.4.14) según la completion recibida en
    /// `kind`/`value`: empuja el resultado interno a devolver tal cual, o salta a `done`
    /// con el valor final.
    YieldStarStep {
        slot: u32,
        kind_slot: u32,
        value_slot: u32,
        done: u32,
    },
    /// Suspende devolviendo el objeto resultado sin envolver; al reanudar guarda la
    /// completion recibida en `kind`/`value`.
    YieldRaw {
        kind_slot: u32,
        value_slot: u32,
    },
    Await,

    // --- Funciones ---
    /// Crea el objeto `arguments` (mapeado si hay tabla de parámetros).
    CreateArguments(Option<u32>),
    GetArg(u32),
    RestArgs(u32),

    // --- Clases ---
    /// [superclass] → [protoParent ctorParent]; sin `extends`, `None`.
    ClassHeritage(bool),
    /// [protoParent ctorParent] → [F proto]
    ClassCreate {
        code: u32,
        name: FnName,
    },
    /// [F proto (key) fn] → [F proto]: define un método/getter/setter.
    DefineMethod {
        kind: MethodKind,
        is_static: bool,
        name: Option<u32>,
    },
    /// [F proto key] → [F proto key']: ToPropertyKey de una clave de campo calculada.
    /// [F proto key fn?] → [F proto]: agrega un campo de instancia a `[[Fields]]`.
    AddField {
        name: Option<u32>,
        private: bool,
        has_init: bool,
    },
    /// [F proto name fn] → [F proto]: agrega un método privado de instancia.
    AddPrivateMethod(MethodKind),
    /// [obj name fn] → [obj]: PrivateMethodOrAccessorAdd (estáticos).
    AddPrivateMethodTo(MethodKind),
    /// [obj key fn?] → [obj]: DefineField (campo estático) con el inicializador.
    DefineStaticField {
        private: bool,
        has_init: bool,
    },
    /// [obj fn] → [obj]: Call(fn, obj) (bloque estático).
    CallStaticBlock,
    NewPrivateName(u32),

    // --- Declaraciones globales y de eval ---
    /// [fn] → (): CreateGlobalFunctionBinding / el binding en el varEnv de eval.
    DeclareFunction(u32),
    /// CreateGlobalVarBinding / CreateMutableBinding en el varEnv de eval.
    DeclareVar(u32),
    /// Binding léxico global (CreateMutableBinding/CreateImmutableBinding).
    DeclareGlobalLex {
        name: u32,
        constant: bool,
    },
    /// [valor] → (): InitializeBinding en el Declarative Record global.
    InitGlobalLex(u32),
    /// [valor] → (): SetMutableBinding en el VariableEnvironment (B.3.2.2/B.3.2.3).
    SetVarEnv(u32),
    /// Empuja si la función de bloque `name` se elevó (B.3.2.2/B.3.2.3, decidido en
    /// GlobalDeclarationInstantiation/EvalDeclarationInstantiation).
    IsHoisted(u32),
}

/// Una función compilada.
#[derive(Debug)]
pub struct FunctionCode {
    pub name: JsString,
    pub kind: CodeKind,
    pub strict: bool,
    pub is_generator: bool,
    pub is_async: bool,
    pub ops: Vec<Op>,
    pub consts: Vec<Constant>,
    pub num_locals: u32,
    /// ExpectedArgumentCount (la propiedad `length`).
    pub length: u32,
    pub source: Rc<Source>,
    /// El texto de la función para `Function.prototype.toString`.
    pub span: Span,
    /// (pc, offset en la fuente) en orden creciente de pc.
    pub positions: Vec<(u32, u32)>,
    /// ¿Hace falta un objeto prototype (funciones normales y generadores)?
    pub has_prototype: bool,
    /// Datos para la instanciación de las declaraciones (script y eval).
    pub decls: Option<Box<Declarations>>,
    /// ¿La función tiene `eval` directo o `with`? (Solo informativo.)
    pub dynamic: bool,
}

impl FunctionCode {
    /// El offset fuente de `pc` (para los mensajes y `stack`).
    pub fn position(&self, pc: usize) -> Option<u32> {
        let pc = pc as u32;
        match self.positions.binary_search_by(|(p, _)| p.cmp(&pc)) {
            Ok(i) => Some(self.positions[i].1),
            Err(0) => None,
            Err(i) => Some(self.positions[i - 1].1),
        }
    }
}

/// Lo que GlobalDeclarationInstantiation (§16.1.7) y EvalDeclarationInstantiation
/// (§19.2.1.3) verifican antes de ejecutar el código.
#[derive(Debug, Clone, Default)]
pub struct Declarations {
    /// VarDeclaredNames sin las funciones.
    pub var_names: Vec<Atom>,
    /// Las funciones a instanciar (último por nombre).
    pub function_names: Vec<Atom>,
    /// Declaraciones léxicas (nombre, ¿const?).
    pub lex_names: Vec<(Atom, bool)>,
    /// Funciones en bloques candidatas a B.3.2.2 / B.3.2.3.
    pub annex_b: Vec<Atom>,
    /// Para eval: ¿el código es estricto? (Entonces su varEnv es propio.)
    pub strict: bool,
    /// Para eval: el layout de su LexicalEnvironment si hay que materializarlo.
    pub top_layout: Option<Rc<ScopeLayout>>,
}
