//! Right-hand info panel: the native replacement for the compact widgets of
//! the web app (context, plan, model quota, session, agent, notes).

mod notes;
mod quota;
mod sections;

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::workspace::Workspace;
use notes::Notes;
use quota::QuotaState;

const PANEL_WIDTH: f32 = 300.;
const QUOTA_REFRESH: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Context,
    Plan,
    Quota,
    Session,
    Agent,
    Notes,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Self::Context => "Контекст",
            Self::Plan => "План",
            Self::Quota => "Лимиты модели",
            Self::Session => "Сессия",
            Self::Agent => "Агент",
            Self::Notes => "Заметки",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Context => "panel-context",
            Self::Plan => "panel-plan",
            Self::Quota => "panel-quota",
            Self::Session => "panel-session",
            Self::Agent => "panel-agent",
            Self::Notes => "panel-notes",
        }
    }
}

#[derive(Default)]
pub(super) struct InfoPanel {
    pub open: bool,
    collapsed: HashSet<Section>,
    show_tools: bool,
    quota: QuotaState,
    quota_timer: Option<Task<()>>,
    notes: Option<Notes>,
}

impl Workspace {
    pub(super) fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel.open = !self.panel.open;
        if self.panel.open {
            if self.panel.notes.is_none() {
                self.panel.notes = Some(Notes::new(window, cx));
            }
            self.load_quota(cx);
            self.panel.quota_timer = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(QUOTA_REFRESH).await;
                    if this.update(cx, |this, cx| this.load_quota(cx)).is_err() {
                        break;
                    }
                }
            }));
        } else {
            self.panel.quota_timer = None;
        }
        cx.notify();
    }

    fn toggle_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.panel.collapsed.remove(&section) {
            self.panel.collapsed.insert(section);
        }
        cx.notify();
    }

    pub(super) fn render_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.panel.open || self.settings.is_some() {
            return None;
        }
        let theme = cx.theme();
        let (border, background) = (theme.border, theme.sidebar);
        let (context_summary, context) = self.context_section(cx);
        let (plan_summary, plan) = self.plan_section(cx);
        let (quota_summary, quota) = self.quota_section(cx);
        let session = self.session_section(cx);
        let agent = self.agent_section(cx);
        let notes = self.panel.notes.as_ref().map(|notes| notes.render(cx));
        Some(
            v_flex()
                .id("info-panel")
                .w(px(PANEL_WIDTH))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_l_1()
                .border_color(border)
                .bg(background)
                .child(self.section(Section::Context, context_summary, context, cx))
                .child(self.section(Section::Plan, plan_summary, plan, cx))
                .child(self.section(Section::Quota, quota_summary, quota, cx))
                .child(self.section(Section::Session, None, session, cx))
                .child(self.section(Section::Agent, None, agent, cx))
                .children(notes.map(|notes| self.section(Section::Notes, None, notes, cx))),
        )
    }

    fn section(
        &self,
        section: Section,
        summary: Option<SharedString>,
        body: AnyElement,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let collapsed = self.panel.collapsed.contains(&section);
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        v_flex()
            .flex_none()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .id(section.id())
                    .px_4()
                    .py_2()
                    .gap_2()
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall()
                        .text_color(muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(section.title()),
                    )
                    .children(
                        summary.map(|summary| div().text_xs().text_color(muted).child(summary)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(section, cx))),
            )
            .when(!collapsed, |this| {
                this.child(div().px_4().pb_3().child(body))
            })
    }
}

/// Label on the left, value on the right.
fn row(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .w_full()
        .gap_3()
        .justify_between()
        .text_sm()
        .child(
            div()
                .flex_none()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().min_w_0().truncate().child(value.into()))
}

fn muted(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
