//! Motor JavaScript conforme a ECMA-262: el front-end (lexer, parser, AST con la forma de
//! ESTree y los early errors del spec), el compilador a bytecode, el intérprete con su heap y
//! recolector de basura, y los built-ins.

#![forbid(unsafe_code)]

pub mod abstract_ops;
pub mod ast;
pub mod bigint;
pub mod builtins;
pub mod compiler;
pub mod estree;
pub mod heap;
pub mod host;
pub mod lexer;
pub mod object;
pub mod parser;
pub mod regexp;
pub mod string;
pub mod value;
pub mod vm;

pub use host::{JsError, Runtime};
pub use parser::{ParseOptions, SyntaxError};

/// Parsea `src` con el símbolo inicial Script (§16.1).
pub fn parse_script(src: &str, opts: &ParseOptions) -> Result<ast::Program, SyntaxError> {
    parser::Parser::new(src, false, opts)?.parse_program()
}

/// Parsea `src` con el símbolo inicial Module (§16.2): siempre estricto, con `import`/`export`
/// y `await` en el top-level.
pub fn parse_module(src: &str, opts: &ParseOptions) -> Result<ast::Program, SyntaxError> {
    parser::Parser::new(src, true, opts)?.parse_program()
}
