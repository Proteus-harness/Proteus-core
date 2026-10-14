//! Headless agent settings: schema-driven fields, the editable draft and the
//! presentation catalog rendered by `ui::settings`.

pub mod addons;
pub mod catalog;
mod draft;
pub mod history;
mod schema;

pub use draft::{Draft, FieldPath};
pub use schema::{
    FieldKind, TextFormat, choice_title, default_hint, field_kind, format_value, parse_text,
};

#[cfg(test)]
mod tests;
