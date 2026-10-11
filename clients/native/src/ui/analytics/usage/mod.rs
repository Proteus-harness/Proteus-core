//! «Расход и контекст»: request usage with cost estimate and the context
//! window snapshot of the chosen session.

mod context;
mod requests;

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppContextMapSnapshot;
use proteus_contracts::domain::SessionUsageSnapshot;

use super::picker::SessionPicker;
use super::widgets::{Fetch, error_line, header, refresh_button, segments};
use crate::analytics::usage::{Order, Period, StatusFilter};
use crate::api::Api;

const USAGE_POLL: Duration = Duration::from_secs(5);
const CONTEXT_POLL: Duration = Duration::from_secs(7);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Requests,
    Context,
}

pub(super) struct UsagePage {
    api: Api,
    picker: Entity<SessionPicker>,
    tab: Tab,
    usage: Fetch<Option<SessionUsageSnapshot>>,
    context: Fetch<AppContextMapSnapshot>,
    period: Period,
    model: Option<String>,
    status: StatusFilter,
    order: Order,
    query: Entity<InputState>,
    page: usize,
    expanded: HashSet<usize>,
    _subscriptions: Vec<Subscription>,
}

impl UsagePage {
    pub fn new(
        api: Api,
        picker: Entity<SessionPicker>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("ID запроса, хода или модель"));
        let subscriptions = vec![
            cx.observe(&picker, |this, _, cx| {
                this.usage.reset();
                this.context.reset();
                this.period = Period::All;
                this.model = None;
                this.page = 0;
                this.expanded.clear();
                cx.notify();
            }),
            cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.page = 0;
                    cx.notify();
                }
            }),
        ];
        Self {
            api,
            picker,
            tab: Tab::Requests,
            usage: Fetch::default(),
            context: Fetch::default(),
            period: Period::All,
            model: None,
            status: StatusFilter::All,
            order: Order::Oldest,
            query,
            page: 0,
            expanded: HashSet::new(),
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let dir = self.picker.read(cx).selected.clone();
        let api = self.api.clone();
        match self.tab {
            Tab::Requests => self
                .usage
                .start(move || api.usage(&dir), |this| &mut this.usage, cx),
            Tab::Context => {
                self.context
                    .start(move || api.context(&dir), |this| &mut this.context, cx)
            }
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Requests => {
                if self.usage.stale(Some(USAGE_POLL)) {
                    self.refresh(cx);
                }
                self.usage.poll(USAGE_POLL, |this| &mut this.usage, cx);
            }
            Tab::Context => {
                if self.context.stale(Some(CONTEXT_POLL)) {
                    self.refresh(cx);
                }
                self.context
                    .poll(CONTEXT_POLL, |this| &mut this.context, cx);
            }
        }
    }
}

impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll(cx);
        let loading = match self.tab {
            Tab::Requests => self.usage.loading,
            Tab::Context => self.context.loading,
        };
        let error = match self.tab {
            Tab::Requests => self.usage.error.clone(),
            Tab::Context => self.context.error.clone(),
        };
        let selected = match self.tab {
            Tab::Requests => 0,
            Tab::Context => 1,
        };
        let body = match self.tab {
            Tab::Requests => self.render_requests(cx),
            Tab::Context => self.render_context(cx),
        };
        v_flex()
            .w_full()
            .gap_4()
            .child(header(
                "Расход и контекст",
                "Запросы модели, оценка стоимости и окно контекста выбранной сессии.",
                [
                    SessionPicker::render(&self.picker, cx),
                    refresh_button("usage-refresh", loading, cx, Self::refresh),
                ],
                cx,
            ))
            .child(segments(
                "usage-tabs",
                &["Запросы и расход", "Контекст и инструменты"],
                selected,
                cx,
                |this, index, cx| {
                    this.tab = if index == 0 {
                        Tab::Requests
                    } else {
                        Tab::Context
                    };
                    cx.notify();
                },
            ))
            .children(error.map(|error| error_line(error, cx)))
            .child(body)
    }
}

pub(super) fn query_input(state: &Entity<InputState>) -> Input {
    Input::new(state).small().cleanable(true)
}
