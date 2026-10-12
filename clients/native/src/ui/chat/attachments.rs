//! Images of the composer draft: picked or dropped files and clipboard
//! pastes, checked against the host limits before they are sent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::domain::{
    ImageAttachment, MAX_IMAGE_BYTES, MAX_INPUT_IMAGES, image_mime_type,
};

use super::Chat;
use crate::ui::tip::Tip as _;
use crate::ui::{kit, prefs};

const THUMB: f32 = 56.;

/// An image of the draft with the bytes its preview decodes.
pub(super) struct Attachment {
    image: ImageAttachment,
    preview: Arc<Image>,
}

#[derive(Default)]
pub(super) struct Attachments {
    items: Vec<Attachment>,
}

impl Attachments {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn take(&mut self) -> Vec<ImageAttachment> {
        self.items.drain(..).map(|item| item.image).collect()
    }

    fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
        }
    }

    /// Adds an image the host accepts; the error says why it would not.
    fn add(&mut self, name: String, bytes: Vec<u8>) -> Result<(), String> {
        let format = image_mime_type(&bytes)
            .and_then(ImageFormat::from_mime_type)
            .ok_or_else(|| format!("{name}: поддерживаются PNG, JPEG, WebP и GIF"))?;
        if self.items.len() >= MAX_INPUT_IMAGES {
            return Err(format!(
                "{name}: к сообщению можно приложить не больше {MAX_INPUT_IMAGES} изображений"
            ));
        }
        let total: usize = self.items.iter().map(|item| item.preview.bytes.len()).sum();
        if total + bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!("{name}: изображения сообщения вместе больше 5 МиБ"));
        }
        let image = ImageAttachment::from_bytes(name, &bytes).map_err(|error| error.to_string())?;
        self.items.push(Attachment {
            image,
            preview: Arc::new(Image::from_bytes(format, bytes)),
        });
        Ok(())
    }
}

/// A file as an attachment candidate; oversized files are not read.
fn read_file(path: &Path) -> Result<(String, Vec<u8>), String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let size = std::fs::metadata(path)
        .map_err(|error| format!("{name}: {error}"))?
        .len();
    if size > MAX_IMAGE_BYTES as u64 {
        return Err(format!("{name}: файл больше 5 МиБ"));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("{name}: {error}"))?;
    Ok((name, bytes))
}

impl Chat {
    pub(super) fn attach_files(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let files = paths.iter().map(|path| read_file(path)).collect();
        self.attach(files, cx);
    }

    /// Takes images and copied files from a paste; text, even beside an
    /// image, stays for the input.
    pub(super) fn attach_clipboard(
        &mut self,
        item: &ClipboardItem,
        cx: &mut Context<Self>,
    ) -> bool {
        if item.text().is_some_and(|text| !text.is_empty()) {
            return false;
        }
        let mut files = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let name = format!("clipboard.{}", image.format.extension());
                    files.push(Ok((name, image.bytes.clone())));
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    files.extend(paths.paths().iter().map(|path| read_file(path)));
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if files.is_empty() {
            return false;
        }
        self.attach(files, cx);
        true
    }

    pub(super) fn choose_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Прикрепить".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update(cx, |this, cx| this.attach_files(&paths, cx))
                .ok();
        })
        .detach();
    }

    fn attach(&mut self, files: Vec<Result<(String, Vec<u8>), String>>, cx: &mut Context<Self>) {
        let errors: Vec<String> = files
            .into_iter()
            .filter_map(|file| {
                file.and_then(|(name, bytes)| self.attachments.add(name, bytes))
                    .err()
            })
            .collect();
        if !errors.is_empty() {
            self.show_notice(errors.join("\n"), cx);
        }
        cx.notify();
    }

    pub(super) fn render_attachments(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let thumbs = self
            .attachments
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                div()
                    .id(("attachment", index))
                    .group("attachment")
                    .relative()
                    .size(prefs::scaled(THUMB))
                    .rounded(theme.radius * 1.5)
                    .border_1()
                    .border_color(theme.border)
                    .tip_text(item.image.name.clone())
                    .child(
                        img(item.preview.clone())
                            .size_full()
                            .rounded(theme.radius * 1.5)
                            .object_fit(ObjectFit::Cover),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0p5()
                            .right_0p5()
                            .invisible()
                            .group_hover("attachment", |style| style.visible())
                            .child(
                                kit::button(("attachment-remove", index))
                                    .icon(IconName::Close)
                                    .tooltip("Убрать")
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.attachments.remove(index);
                                        cx.notify();
                                    })),
                            ),
                    )
            });
        Some(
            h_flex()
                .flex_wrap()
                .gap_2()
                .px_1()
                .pt_1()
                .children(thumbs)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use proteus_contracts::domain::{MAX_IMAGE_BYTES, MAX_INPUT_IMAGES};

    use super::Attachments;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn png(size: usize) -> Vec<u8> {
        let mut bytes = PNG.to_vec();
        bytes.resize(size.max(PNG.len()), 0);
        bytes
    }

    #[test]
    fn draft_takes_what_the_host_accepts() {
        let mut draft = Attachments::default();
        assert!(draft.add("notes.txt".into(), b"plain".to_vec()).is_err());
        for index in 0..MAX_INPUT_IMAGES {
            draft.add(format!("{index}.png"), png(16)).unwrap();
        }
        assert!(draft.add("extra.png".into(), png(16)).is_err());

        draft.remove(0);
        assert!(draft.add("big.png".into(), png(MAX_IMAGE_BYTES)).is_err());
        draft
            .add("fits.png".into(), png(MAX_IMAGE_BYTES - 48))
            .unwrap();

        let images = draft.take();
        assert_eq!(images.len(), MAX_INPUT_IMAGES);
        assert_eq!(images[0].mime_type, "image/png");
        assert!(draft.is_empty());
    }
}
