//! Settings navigation: agent pages, then client pages.

use std::collections::HashSet;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::widgets::changed_dot;
use super::{SettingsEvent, SettingsView};
use crate::settings::catalog::{
    CLIENT_PAGES, OVERVIEW_PAGES, Overview, PageId, agent_pages, area_page, page_title,
};
use crate::ui::prefs;
use crate::ui::tip::Tip as _;

const NAV_WIDTH: f32 = 232.;

fn page_icon(page: &PageId) -> Icon {
    match page {
        PageId::Model => Icon::new(Lucide::Brain),
        PageId::Slot(slot) => match slot.as_str() {
            "workflow" => Icon::new(Lucide::Workflow),
            "context" => Icon::new(Lucide::Layers),
            "compactor" => Icon::new(Lucide::Shrink),
            "tool_exposure" => Icon::new(IconName::Eye),
            "policy" => Icon::new(Lucide::Shield),
            _ => Icon::new(Lucide::Puzzle),
        },
        PageId::ContextProviders => Icon::new(IconName::BookOpen),
        PageId::Tools => Icon::new(Lucide::Wrench),
        PageId::Hooks => Icon::new(Lucide::Webhook),
        PageId::Plugins => Icon::new(Lucide::Package),
        PageId::History => Icon::new(Lucide::Clock),
        PageId::Appearance => Icon::new(IconName::Palette),
        PageId::Chat => Icon::new(Lucide::MessageSquare),
        PageId::Shortcuts => Icon::new(Lucide::Keyboard),
        PageId::Extensions => Icon::new(Lucide::Puzzle),
        PageId::Overview(Overview::Usage) => Icon::new(Lucide::Gauge),
        PageId::Overview(Overview::Analysis) => Icon::new(Lucide::ListChecks),
        PageId::Overview(Overview::Architecture) => Icon::new(Lucide::Network),
    }
}

impl SettingsView {
    fn agent_page_list(&self) -> Vec<PageId> {
        self.snapshot()
            .map(agent_pages)
            .unwrap_or_else(|| vec![PageId::Model])
    }

    /// Moves the selection through the navigation list, wrapping around.
    pub(super) fn step_page(&mut self, offset: isize, cx: &mut Context<Self>) {
        let mut pages = self.agent_page_list();
        pages.extend(CLIENT_PAGES.iter().cloned());
        pages.extend(OVERVIEW_PAGES.iter().cloned());
        let current = pages
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(pages.len() as isize) as usize;
        self.select_page(pages[next].clone(), cx);
    }

    pub(super) fn render_nav(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let snapshot = self.snapshot();
        let agent = self.agent_page_list();
        let changed: HashSet<PageId> = self
            .draft
            .areas(&self.baseline)
            .iter()
            .map(area_page)
            .collect();
        let item = |page: PageId, cx: &Context<Self>| {
            let selected = self.page == page;
            let title = page_title(snapshot, &page);
            let dot = changed.contains(&page);
            let target = page.clone();
            h_flex()
                .id(ElementId::Name(format!("settings-nav-{page:?}").into()))
                .h_8()
                .px_2()
                .gap_2()
                .items_center()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .map(|item| {
                    if selected {
                        item.bg(theme.sidebar_accent)
                            .text_color(theme.sidebar_accent_foreground)
                    } else {
                        item.text_color(theme.sidebar_foreground)
                            .hover(|item| item.bg(theme.list_hover))
                    }
                })
                .child(page_icon(&page).small().text_color(if selected {
                    theme.sidebar_accent_foreground
                } else {
                    theme.muted_foreground
                }))
                .child(div().flex_1().min_w_0().truncate().child(title))
                .when(dot, |item| item.child(changed_dot(cx)))
                .on_click(cx.listener(move |this, _, _, cx| this.select_page(target.clone(), cx)))
        };
        let heading = |text: &'static str| {
            div()
                .px_2()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let profile = snapshot.and_then(|snapshot| {
            snapshot
                .target_path
                .clone()
                .or_else(|| snapshot.config_path.clone())
                .map(|path| (path, snapshot.writable))
        });
        v_flex()
            .w(prefs::scaled(NAV_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex().h(prefs::scaled(44.)).px_2().items_center().child(
                    Button::new("settings-back")
                        .icon(IconName::ArrowLeft)
                        .label("Чат")
                        .ghost()
                        .small()
                        .tooltip("Вернуться к разговору (Ctrl+,)")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                ),
            )
            .child(
                div()
                    .px_4()
                    .pb_1()
                    .text_lg()
                    .font_semibold()
                    .child("Настройки"),
            )
            .child(
                v_flex()
                    .id("settings-pages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_0p5()
                    .child(heading("Агент"))
                    .children(agent.into_iter().map(|page| item(page, cx)))
                    .child(heading("Клиент"))
                    .children(CLIENT_PAGES.iter().cloned().map(|page| item(page, cx)))
                    .child(heading("Обзор"))
                    .children(OVERVIEW_PAGES.iter().cloned().map(|page| item(page, cx))),
            )
            .children(profile.map(|(path, writable)| {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                v_flex()
                    .id("settings-profile")
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
                    .when(!writable, |footer| {
                        footer.child(div().text_color(theme.warning).child("Только чтение"))
                    })
                    .tip_text(path)
            }))
    }
}
