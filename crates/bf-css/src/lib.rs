//! Frente CSS del motor: CSS Syntax 3, Selectors 4, Media Queries 4 y el modelo de hoja de
//! estilos (reglas, at-rules, nesting y recolección de declaraciones).

#![forbid(unsafe_code)]

pub mod media;
pub mod query; // E4: matches, closest y querySelector del DOM
pub mod selectors;
pub mod stylesheet;
pub mod syntax;
