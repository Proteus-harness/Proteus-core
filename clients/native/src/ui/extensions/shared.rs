//! Data models shared by the views of one extension, e.g. its panel widget
//! and its icon, so both show the same data and fetch it once.

use std::any::TypeId;
use std::collections::HashMap;

use gpui_kit::*;

/// Held weakly: a model lives while some view keeps it.
#[derive(Default)]
struct SharedModels(HashMap<TypeId, AnyWeakEntity>);

impl Global for SharedModels {}

pub(super) fn shared<T: 'static>(
    cx: &mut App,
    create: impl FnOnce(&mut App) -> Entity<T>,
) -> Entity<T> {
    let key = TypeId::of::<T>();
    let existing = cx
        .try_global::<SharedModels>()
        .and_then(|models| models.0.get(&key))
        .and_then(AnyWeakEntity::upgrade)
        .and_then(|entity| entity.downcast::<T>().ok());
    if let Some(entity) = existing {
        return entity;
    }
    let entity = create(cx);
    cx.default_global::<SharedModels>()
        .0
        .insert(key, entity.downgrade().into());
    entity
}
