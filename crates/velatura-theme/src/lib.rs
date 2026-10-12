//! Theme model and colour maths for Velatura.
//!
//! This crate performs no I/O. Everything a theme file contributes to a
//! generated config passes through [`Color`], which can only be turned into
//! text by a closed set of formatters.

mod accent;
mod color;

pub use accent::{Accent, nearest_accent};
pub use color::{Color, ParseColorError};
