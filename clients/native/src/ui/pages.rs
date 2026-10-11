//! Extension pages shown in place of the chat, and the tabs that switch
//! between them.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use super::extensions::{self, Host};
use super::workspace::Workspace;

impl Workspace {
    /// Shows the page of an enabled extension, building it on first use.
    pub(super) fn open_page(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyView> {
        let surface = extensions::find(id)?.page.as_ref()?;
        if !extensions::is_enabled(id, cx) {
            return None;
        }
        self.close_settings(cx);
        let page = match self.pages.get(id) {
            Some(page) => page.clone(),
            None => {
                let page = (surface.build)(Host::new(cx.weak_entity()), window, cx);
                self.pages.insert(id, page.clone());
                page
            }
        };
        self.page = Some(id);
        page.focus.focus(window, cx);
        cx.notify();
        Some(page.view)
    }

    pub(super) fn show_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = None;
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Cycles through "Чат" and the enabled pages.
    pub(super) fn step_tab(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let mut tabs: Vec<Option<&'static str>> = vec![None];
        tabs.extend(
            extensions::visible_pages(cx)
                .into_iter()
                .map(|manifest| Some(manifest.id)),
        );
        let current = tabs.iter().position(|tab| *tab == self.page).unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(tabs.len() as isize) as usize;
        match tabs[next] {
            Some(id) => {
                self.open_page(id, window, cx);
            }
            None => self.show_chat(window, cx),
        }
    }

    /// The open page, dropping pages of extensions switched off meanwhile.
    pub(super) fn current_page(&mut self, cx: &App) -> Option<(&'static str, AnyView)> {
        self.pages.retain(|id, _| extensions::is_enabled(id, cx));
        let id = self.page?;
        match self.pages.get(id) {
            Some(page) => Some((id, page.view.clone())),
            None => {
                self.page = None;
                None
            }
        }
    }

    /// "Чат" plus one tab per enabled page; nothing when there are no pages.
    pub(super) fn render_page_tabs(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let pages = extensions::visible_pages(cx);
        if pages.is_empty() {
            return None;
        }
        let chat = Button::new("tab-chat")
            .ghost()
            .small()
            .icon(Icon::new(gpui_kit::assets::IconName::MessageSquare))
            .label("Чат")
            .selected(self.page.is_none())
            .on_click(cx.listener(|this, _, window, cx| this.show_chat(window, cx)));
        Some(
            h_flex()
                .gap_1()
                .child(chat)
                .children(pages.into_iter().enumerate().filter_map(|(ix, manifest)| {
                    let surface = manifest.page.as_ref()?;
                    let id = manifest.id;
                    Some(
                        Button::new(("tab-page", ix))
                            .ghost()
                            .small()
                            .icon((manifest.icon)())
                            .label(surface.title)
                            .selected(self.page == Some(id))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_page(id, window, cx);
                            })),
                    )
                })),
        )
    }
}
