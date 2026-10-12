//! Images of sent messages, fetched from the session's image store the first
//! time their row is shown. Thumbnails keep one size while they load, so a
//! row never changes its measured height.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::domain::{ImageRef, image_mime_type};

use super::Chat;
use crate::ui::prefs;
use crate::ui::tip::Tip as _;

const THUMB: f32 = 96.;

enum Stored {
    Loading,
    Ready(Arc<Image>),
    Failed(SharedString),
}

#[derive(Default)]
pub(super) struct SentImages {
    by_id: HashMap<String, Stored>,
}

impl SentImages {
    /// Images of a row that were never requested.
    pub fn missing(&self, images: &[ImageRef]) -> Vec<String> {
        images
            .iter()
            .filter(|image| !self.by_id.contains_key(&image.id))
            .map(|image| image.id.clone())
            .collect()
    }
}

impl Chat {
    pub(super) fn fetch_images(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        for id in ids {
            self.sent_images.by_id.insert(id.clone(), Stored::Loading);
            let (api, dir) = (self.api.clone(), self.session.dir.clone());
            cx.spawn(async move |this, cx| {
                let request = id.clone();
                let fetched = cx
                    .background_executor()
                    .spawn(async move { api.image(&dir, &request) })
                    .await;
                let stored = match fetched {
                    Ok(bytes) => {
                        match image_mime_type(&bytes).and_then(ImageFormat::from_mime_type) {
                            Some(format) => {
                                Stored::Ready(Arc::new(Image::from_bytes(format, bytes)))
                            }
                            None => Stored::Failed("неизвестный формат".into()),
                        }
                    }
                    Err(error) => Stored::Failed(format!("{error:#}").into()),
                };
                this.update(cx, |this, cx| {
                    this.sent_images.by_id.insert(id, stored);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }
}

pub(super) fn render_sent_images(
    images: &[ImageRef],
    store: &SentImages,
    cx: &App,
) -> Option<AnyElement> {
    if images.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let thumbs = images.iter().map(|image| {
        let frame = div()
            .id(ElementId::Name(image.id.clone().into()))
            .size(prefs::scaled(THUMB))
            .flex_none()
            .rounded(theme.radius)
            .bg(theme.muted);
        match store.by_id.get(&image.id) {
            Some(Stored::Ready(bytes)) => frame
                .tip_text(image.name.clone())
                .child(
                    img(bytes.clone())
                        .size_full()
                        .rounded(theme.radius)
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            Some(Stored::Failed(reason)) => frame
                .tip_text(format!("{}: {reason}", image.name))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(Lucide::ImageOff)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .into_any_element(),
            Some(Stored::Loading) | None => frame.into_any_element(),
        }
    });
    Some(
        h_flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .children(thumbs)
            .into_any_element(),
    )
}
