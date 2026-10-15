//! Extension manager: order, switch on and off, open the page of an
//! extension, install one from a ZIP archive or delete it, restore the
//! standard list.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::widgets::{Tone, card, chip, disclosure, mono, note, section};
use super::{Feedback, SettingsView};
use crate::preferences::ExtensionPreferences;
use crate::settings::catalog::PageId;
use crate::ui::extensions::{self, Manifest};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

const ZIP_KEY: &str = "extensions-zip";
const RESTORE_KEY: &str = "extensions-restore";

/// Extension row lifted by its grip.
#[derive(Clone)]
pub(super) struct DraggedExtension {
    id: &'static str,
    title: &'static str,
    icon: Lucide,
}

impl Render for DraggedExtension {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.drag_border)
            .bg(theme.popover)
            .text_sm()
            .text_color(theme.popover_foreground)
            .shadow_md()
            .child(Icon::new(self.icon).small())
            .child(self.title)
    }
}

impl SettingsView {
    pub(super) fn extensions_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let rows = extensions::ordered(cx)
            .into_iter()
            .map(|manifest| self.extension_row(manifest, cx).into_any_element())
            .collect::<Vec<_>>();
        let mut parts = self.preferences_problem(cx);
        parts.extend(self.extension_notice(cx));
        parts.push(
            section(
                "Установленные",
                Some(
                    "Включённые расширения получают свою страницу в настройках. \
                     Порядок меняется перетаскиванием за ручку."
                        .into(),
                ),
                None,
                card(rows, cx),
                cx,
            )
            .into_any_element(),
        );
        parts.push(self.install_section(cx).into_any_element());
        parts.push(self.restore_section(cx).into_any_element());
        parts
    }

    /// Installs every dropped or picked ZIP archive and reports the result.
    pub(super) fn install_archives(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let archives: Vec<&PathBuf> = paths.iter().filter(|path| is_zip(path)).collect();
        if archives.is_empty() {
            self.extension_notice = Some(Feedback {
                text: "Нужен ZIP-архив расширения".into(),
                error: true,
            });
            cx.notify();
            return;
        }
        let mut installed = Vec::new();
        let mut errors = Vec::new();
        for archive in archives {
            match extensions::install(archive, cx) {
                Ok(manifest) => installed.push(manifest.title),
                Err(error) => errors.push(format!("{}: {error:#}", file_name(archive))),
            }
        }
        self.extension_notice = Some(if errors.is_empty() {
            Feedback {
                text: format!("Установлено: {}", installed.join(", ")).into(),
                error: false,
            }
        } else {
            Feedback {
                text: errors.join("\n").into(),
                error: true,
            }
        });
        cx.notify();
    }

    fn choose_archive(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Установить".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            this.update(cx, |this, cx| this.install_archives(&paths, cx))
                .ok();
        })
        .detach();
    }

    pub(super) fn remove_extension(&mut self, id: &str, cx: &mut Context<Self>) {
        self.removing = None;
        self.extension_notice = Some(match extensions::remove(id, cx) {
            Ok(()) => Feedback {
                text: "Расширение удалено".into(),
                error: false,
            },
            Err(error) => Feedback {
                text: format!("Не удалено: {error:#}").into(),
                error: true,
            },
        });
        if self.page == PageId::Extension(id.to_string()) {
            self.page = PageId::Extensions;
        }
        cx.notify();
    }

    fn extension_notice(&self, cx: &App) -> Option<AnyElement> {
        let notice = self.extension_notice.as_ref()?;
        let theme = cx.theme();
        let color = if notice.error {
            theme.danger
        } else {
            theme.success
        };
        Some(
            h_flex()
                .gap_2()
                .items_start()
                .px_4()
                .py_3()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(color.opacity(0.4))
                .bg(color.opacity(0.08))
                .text_sm()
                .child(
                    Icon::new(if notice.error {
                        IconName::TriangleAlert
                    } else {
                        IconName::CircleCheck
                    })
                    .small()
                    .flex_none()
                    .mt_0p5()
                    .text_color(color),
                )
                .child(div().flex_1().min_w_0().child(notice.text.clone()))
                .into_any_element(),
        )
    }

    fn extension_row(&self, manifest: &'static Manifest, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let id = manifest.id;
        let enabled = extensions::is_enabled(id, cx);
        let removable = extensions::folder(id).is_some();
        let confirming = self.removing.as_deref() == Some(id);
        let dragged = DraggedExtension {
            id,
            title: manifest.title,
            icon: manifest.icon,
        };
        let grip = div()
            .id(ElementId::Name(format!("extension-grip-{id}").into()))
            .flex_none()
            .px_1()
            .py_1()
            .rounded(theme.radius)
            .cursor_grab()
            .text_color(theme.muted_foreground)
            .hover(|grip| grip.bg(theme.list_hover).text_color(theme.foreground))
            .child(Icon::new(Lucide::GripVertical).small())
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| dragged.clone())
            });
        let text = v_flex()
            .id(ElementId::Name(format!("extension-open-{id}").into()))
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .cursor_pointer()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .text_sm()
                    .child(Icon::new(manifest.icon).small().text_color(if enabled {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    }))
                    .child(manifest.title)
                    .children(surface_chips(manifest, cx))
                    .when(removable, |title| {
                        title.child(chip("установлено", Tone::Accent, cx))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(manifest.description),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_page(PageId::Extension(id.to_string()), window, cx)
            }));
        let controls = h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .map(|controls| {
                if confirming {
                    controls
                        .child(
                            kit::button(ElementId::Name(
                                format!("extension-remove-yes-{id}").into(),
                            ))
                            .label("Удалить")
                            .danger()
                            .small()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove_extension(id, cx)),
                            ),
                        )
                        .child(
                            kit::button(ElementId::Name(
                                format!("extension-remove-no-{id}").into(),
                            ))
                            .label("Отмена")
                            .ghost()
                            .small()
                            .on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.removing = None;
                                    cx.notify();
                                },
                            )),
                        )
                } else {
                    controls
                        .child(
                            kit::switch(ElementId::Name(format!("extension-enabled-{id}").into()))
                                .checked(enabled)
                                .on_click(move |checked: &bool, _, cx| {
                                    let checked = *checked;
                                    prefs::update(cx, |prefs| {
                                        prefs.extensions.set_enabled(id, checked)
                                    });
                                }),
                        )
                        .when(removable, |controls| {
                            controls.child(
                                kit::button(ElementId::Name(
                                    format!("extension-remove-{id}").into(),
                                ))
                                .icon(Lucide::Trash)
                                .ghost()
                                .small()
                                .hint("Удалить", cx)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.removing = Some(id.to_string());
                                        cx.notify();
                                    },
                                )),
                            )
                        })
                }
            })
            .child(
                Icon::new(IconName::ChevronRight)
                    .small()
                    .text_color(theme.muted_foreground),
            );
        h_flex()
            .id(ElementId::Name(format!("extension-row-{id}").into()))
            .px_2()
            .py_2p5()
            .gap_2()
            .items_center()
            .hover(|row| row.bg(theme.list_hover.opacity(0.5)))
            .drag_over::<DraggedExtension>(move |row, dragged, _, cx| {
                if dragged.id == id {
                    row
                } else {
                    row.bg(cx.theme().drop_target)
                }
            })
            .on_drop(cx.listener(move |_, dragged: &DraggedExtension, _, cx| {
                move_extension(dragged.id, id, cx);
            }))
            .child(grip)
            .child(text)
            .child(controls)
    }

    fn install_section(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let open = self.is_open(ZIP_KEY);
        let folder = crate::declarative::manifest::root().map_or_else(
            || "папка настроек не найдена".to_string(),
            |root| root.display().to_string(),
        );
        let content = self.reveal(ZIP_KEY, cx, || {
            v_flex()
                .px_4()
                .pb_4()
                .gap_3()
                .child(
                    v_flex()
                        .id("extensions-zip-drop")
                        .items_center()
                        .gap_2()
                        .px_4()
                        .py_5()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_dashed()
                        .border_color(theme.border)
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .drag_over::<ExternalPaths>(|zone, _, _, cx| {
                            zone.border_color(cx.theme().drag_border)
                                .bg(cx.theme().drop_target)
                        })
                        .child(Icon::new(Lucide::FileArchive).text_color(theme.muted_foreground))
                        .child(
                            "Перетащите ZIP сюда или в любое место этой страницы. \
                             extension.json и все файлы расширения должны лежать в архиве.",
                        )
                        .child(
                            kit::button("extensions-zip-choose")
                                .label("Выбрать ZIP…")
                                .outline()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.choose_archive(cx))),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Расширения распаковываются в папку")
                        .child(mono(folder.clone(), cx)),
                )
                .into_any_element()
        });
        // The toggle and what it opens share one cell, so no divider is
        // left over while it closes.
        let mut rows = vec![
            v_flex()
                .child(disclosure(
                    "extensions-zip-toggle".into(),
                    open,
                    "Установить из ZIP",
                    false,
                    toggler(ZIP_KEY, cx),
                    cx,
                ))
                .children(content)
                .into_any_element(),
        ];
        rows.extend(extensions::problems().into_iter().map(|problem| {
            v_flex()
                .px_4()
                .py_3()
                .gap_1()
                .child(chip(problem.dir.display().to_string(), Tone::Danger, cx))
                .child(div().text_sm().child(problem.error))
                .into_any_element()
        }));
        section(
            "Новое расширение",
            Some(
                "Расширение — архив с extension.json и программой, которая рисует свои \
                 панели виджетами. Установка и удаление применяются сразу."
                    .into(),
            ),
            None,
            card(rows, cx),
            cx,
        )
    }

    fn restore_section(&self, cx: &Context<Self>) -> Div {
        let open = self.is_open(RESTORE_KEY);
        let content = self.reveal(RESTORE_KEY, cx, || {
            h_flex()
                .px_4()
                .pb_4()
                .gap_4()
                .items_center()
                .child(note("Порядок, включение и места значков вернутся к поставляемым. Установленные расширения останутся.", cx).p_0().flex_1())
                .child(
                    kit::button("extensions-restore")
                        .label("Восстановить")
                        .outline()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            prefs::update(cx, |prefs| {
                                prefs.extensions = ExtensionPreferences::default()
                            });
                            this.toggle_open(RESTORE_KEY, cx);
                        })),
                )
                .into_any_element()
        });
        card(
            [v_flex()
                .child(disclosure(
                    "extensions-restore-toggle".into(),
                    open,
                    "Восстановить стандартный список",
                    false,
                    toggler(RESTORE_KEY, cx),
                    cx,
                ))
                .children(content)
                .into_any_element()],
            cx,
        )
    }
}

/// Opens or closes the disclosure `key` of the settings view.
fn toggler(key: &'static str, cx: &Context<SettingsView>) -> impl Fn(&mut App) + 'static {
    let view = cx.weak_entity();
    move |cx| {
        let _ = view.update(cx, |this, cx| this.toggle_open(key, cx));
    }
}

/// Kinds of surfaces an extension adds, as short chips.
fn surface_chips(manifest: &Manifest, cx: &App) -> Vec<Div> {
    let mut chips = Vec::new();
    if let Some(panel) = &manifest.panel {
        chips.push(chip(format!("Панель · {}", panel.title), Tone::Muted, cx));
    }
    if manifest.document.is_some() {
        chips.push(chip("Вкладки", Tone::Muted, cx));
    }
    if manifest.badge.is_some() {
        chips.push(chip("Значок", Tone::Muted, cx));
    }
    if manifest.page.is_some() {
        chips.push(chip("Страница", Tone::Muted, cx));
    }
    chips
}

/// Puts `id` where `target` stands: after it when moving down, before it
/// when moving up.
fn move_extension(id: &'static str, target: &'static str, cx: &mut App) {
    if id == target {
        return;
    }
    let order: Vec<&'static str> = extensions::ordered(cx)
        .into_iter()
        .map(|manifest| manifest.id)
        .collect();
    let (Some(from), Some(to)) = (
        order.iter().position(|item| *item == id),
        order.iter().position(|item| *item == target),
    ) else {
        return;
    };
    let before = if from < to {
        order.get(to + 1).copied()
    } else {
        Some(target)
    };
    let known = extensions::ids();
    prefs::update(cx, |prefs| {
        prefs.extensions.move_extension(&known, id, before)
    });
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().to_string(),
    )
}
