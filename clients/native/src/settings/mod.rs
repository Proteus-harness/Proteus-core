//! Headless agent settings: schema-driven fields, the editable draft and the
//! page layout rendered by `ui::settings`.

mod draft;
mod layout;
mod schema;

pub use draft::{Draft, FieldPath};
pub use layout::{Choice, Group, Item, Page, pages};
pub use schema::{
    FieldKind, TextFormat, choice_title, default_hint, field_kind, format_value, parse_text,
};

#[cfg(test)]
mod tests;
