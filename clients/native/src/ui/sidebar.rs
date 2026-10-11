use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::app_protocol::{AppSessionActivityStatus, AppSessionSummary};

use super::extensions::{self, Host, SurfaceView};
use super::format;
use super::motion;
use super::workspace::Workspace;

pub(super) const SIDEBAR_WIDTH: f32 = 264.;

impl Workspace {
    /// The extension view shown instead of the sessions, if it is enabled.
    fn sidebar_view(&self, cx: &App) -> Option<&'static str> {
        self.sidebar_view.filter(|id| {
            extensions::visible_sidebars(cx)
                .iter()
                .any(|manifest| manifest.id == *id)
        })
    }

    fn ensure_sidebar(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<SurfaceView> {
        if let Some(view) = self.sidebars.get(id) {
            return Some(view.clone());
        }
        let surface = extensions::find(id)?.sidebar.as_ref()?;
        let view = (surface.build)(Host::new(cx.weak_entity()), window, cx);
        self.sidebars.insert(id, view.clone());
        Some(view)
    }

    /// Switches the sidebar to an extension view (`None` = sessions) and
    /// moves the keyboard there.
    pub(super) fn show_sidebar(
        &mut self,
        id: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_view = id;
        if let Some(view) = id.and_then(|id| self.ensure_sidebar(id, window, cx)) {
            view.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Ctrl+Shift+E: to the first extension view and back to the chat.
    pub(super) fn toggle_sidebar_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(first) = extensions::visible_sidebars(cx)
            .first()
            .map(|manifest| manifest.id)
        else {
            return;
        };
        let focused = self
            .sidebars
            .get(first)
            .is_some_and(|view| view.focus.contains_focused(window, cx));
        if self.sidebar_view(cx) == Some(first) && focused {
            self.show_sidebar(None, window, cx);
            self.focus_active_chat(window, cx);
        } else {
            self.close_settings(cx);
            self.show_sidebar(Some(first), window, cx);
        }
    }

    pub(super) fn render_sidebar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let current = self.sidebar_view(cx);
        let content = match current.and_then(|id| self.ensure_sidebar(id, window, cx)) {
            Some(view) => motion::rise_in(
                div().size_full().child(view.view),
                ElementId::Name(format!("sidebar-{}", current.unwrap_or_default()).into()),
            )
            .into_any_element(),
            None => {
                motion::rise_in(self.render_sessions(cx), "sidebar-sessions").into_any_element()
            }
        };
        let theme = cx.theme();
        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(self.render_sidebar_switcher(current, cx))
            .child(div().flex_1().min_h_0().child(content))
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .child(
                        h_flex()
                            .id("open-settings")
                            .w_full()
                            .h_8()
                            .px_2()
                            .gap_2()
                            .rounded(theme.radius)
                            .text_sm()
                            .text_color(theme.sidebar_foreground)
                            .cursor_pointer()
                            .hover(|item| item.bg(theme.list_hover))
                            .child(
                                Icon::new(IconName::Settings)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .child("Настройки")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
    }

    /// «Сессии» and the views of extensions, plus the new-session button.
    fn render_sidebar_switcher(
        &self,
        current: Option<&'static str>,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let views = extensions::visible_sidebars(cx);
        let mut row = h_flex().flex_none().px_2().py_2().gap_1();
        if !views.is_empty() {
            row = row.child(
                Button::new("sidebar-sessions")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::MessageSquare))
                    .label("Сессии")
                    .selected(current.is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_sidebar(None, window, cx)),
                    ),
            );
            for manifest in views {
                let id = manifest.id;
                let title = manifest
                    .sidebar
                    .as_ref()
                    .map_or(manifest.title, |sidebar| sidebar.title);
                row = row.child(
                    Button::new(ElementId::Name(format!("sidebar-{id}").into()))
                        .ghost()
                        .small()
                        .icon((manifest.icon)())
                        .label(title)
                        .selected(current == Some(id))
                        .tooltip("Ctrl+Shift+E")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.show_sidebar(Some(id), window, cx)
                        })),
                );
            }
        }
        row.child(div().flex_1()).when(current.is_none(), |row| {
            row.child(
                Button::new("new-session")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("Новая сессия (Ctrl+N)")
                    .on_click(cx.listener(|this, _, window, cx| this.new_session(window, cx))),
            )
        })
    }

    fn render_sessions(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let current = self.active_chat_dir();
        let workspace_name = self
            .workspace_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.workspace_path.display().to_string());
        let items = self.sessions.iter().enumerate().map(|(ix, summary)| {
            let selected = current == Some(summary.session_dir.as_path());
            self.render_session_item(ix, summary, selected, cx)
        });
        v_flex()
            .size_full()
            .child(
                div()
                    .px_4()
                    .pb_1()
                    .text_xs()
                    .font_semibold()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(workspace_name),
            )
            .child(
                v_flex()
                    .id("sessions")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_0p5()
                    .children(items),
            )
    }

    fn render_session_item(
        &self,
        ix: usize,
        summary: &AppSessionSummary,
        selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let dir = summary.session_dir.clone();
        let status = summary
            .activity
            .as_ref()
            .map(|activity| activity.status.clone());
        let indicator = match status {
            Some(AppSessionActivityStatus::Running) => {
                Some(Spinner::new().xsmall().into_any_element())
            }
            Some(
                AppSessionActivityStatus::WaitingApproval | AppSessionActivityStatus::WaitingInput,
            ) => Some(
                Icon::new(IconName::CircleAlert)
                    .xsmall()
                    .text_color(theme.warning)
                    .into_any_element(),
            ),
            _ => None,
        };

        h_flex()
            .id(("session", ix))
            .px_2()
            .py_1p5()
            .gap_2()
            .rounded(theme.radius)
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .child(format::session_title(summary)),
            )
            .children(indicator)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format::relative_time(summary.updated_at_ms)),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.select_session(dir.clone(), event.click_count(), window, cx)
            }))
    }
}
