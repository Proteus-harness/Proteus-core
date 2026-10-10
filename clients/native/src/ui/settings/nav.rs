//! Settings navigation: back to chat, search and the page list.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{SettingsEvent, SettingsView};
use crate::settings::{Page, PageIcon};

const NAV_WIDTH: f32 = 248.;

impl SettingsView {
    /// Selected page: the remembered id when it still exists, else the first.
    pub(super) fn current_page<'a>(&self, pages: &'a [Page]) -> Option<&'a Page> {
        self.page
            .as_deref()
            .and_then(|id| pages.iter().find(|page| page.id == id))
            .or(pages.first())
    }

    pub(super) fn render_nav(&self, pages: &[Page], cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let current = self
            .current_page(pages)
            .filter(|_| self.query.is_empty())
            .map(|page| page.id.clone());
        let items = pages.iter().enumerate().map(|(ix, page)| {
            let selected = current.as_deref() == Some(page.id.as_str());
            let changed = page.changed(&self.draft, &self.baseline);
            let id = page.id.clone();
            h_flex()
                .id(("settings-nav", ix))
                .h_8()
                .px_2()
                .gap_2()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(selected, |item| {
                    item.bg(theme.sidebar_accent)
                        .text_color(theme.sidebar_accent_foreground)
                })
                .when(!selected, |item| {
                    item.text_color(theme.sidebar_foreground)
                        .hover(|item| item.bg(theme.list_hover))
                })
                .child(page_icon(page.icon).small().text_color(if selected {
                    theme.sidebar_accent_foreground
                } else {
                    theme.muted_foreground
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(page.title.clone()),
                )
                .when(changed, |item| item.child(changed_dot(cx)))
                .on_click(cx.listener(move |this, _, window, cx| this.select_page(&id, window, cx)))
        });

        v_flex()
            .w(px(NAV_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex().h(px(44.)).flex_none().px_2().child(
                    Button::new("settings-back")
                        .icon(IconName::ArrowLeft)
                        .label("Чат")
                        .ghost()
                        .small()
                        .tooltip("Вернуться к разговору")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                ),
            )
            .child(
                div()
                    .px_4()
                    .pb_3()
                    .text_lg()
                    .font_semibold()
                    .child("Настройки"),
            )
            .when(!pages.is_empty(), |nav| {
                nav.child(
                    div().px_3().pb_3().child(
                        Input::new(&self.search).small().cleanable(true).prefix(
                            Icon::new(IconName::Search)
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        ),
                    ),
                )
            })
            .child(
                v_flex()
                    .id("settings-pages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .gap_0p5()
                    .children(items),
            )
            .children(self.render_nav_footer(cx))
    }

    fn render_nav_footer(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let snapshot = self.snapshot()?;
        let theme = cx.theme();
        let path = snapshot
            .target_path
            .clone()
            .or(snapshot.config_path.clone())?;
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        Some(
            v_flex()
                .id("settings-profile")
                .flex_none()
                .px_4()
                .py_3()
                .gap_0p5()
                .border_t_1()
                .border_color(theme.sidebar_border)
                .text_xs()
                .child(div().text_color(theme.muted_foreground).child("Профиль"))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .truncate()
                        .child(name),
                )
                .when(!snapshot.writable, |footer| {
                    footer.child(div().text_color(theme.warning).child("Только чтение"))
                })
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
                }),
        )
    }
}

pub(super) fn page_icon(icon: PageIcon) -> Icon {
    Icon::new(match icon {
        PageIcon::Model => Lucide::Brain,
        PageIcon::Access => Lucide::Shield,
        PageIcon::Context => Lucide::Layers,
        PageIcon::Tools => Lucide::Wrench,
        PageIcon::Workflow => Lucide::Workflow,
        PageIcon::Hooks => Lucide::Webhook,
        PageIcon::Other => Lucide::Puzzle,
    })
}

/// Marks unsaved edits in navigation, headings and rows.
pub(super) fn changed_dot(cx: &App) -> impl IntoElement + use<> {
    div()
        .size(px(6.))
        .flex_none()
        .rounded_full()
        .bg(cx.theme().warning)
}
