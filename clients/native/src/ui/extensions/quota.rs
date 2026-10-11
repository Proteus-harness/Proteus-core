use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::contracts::{ModelQuotaBucket, ModelQuotaSnapshot};

use super::frame::{is_open, muted, row, widget_frame};
use super::host::{Host, HostEvent};
use crate::ui::format::local_time;

pub(super) const ID: &str = "quota";
const WEEK_SECONDS: u64 = 604_800;
const REFRESH: Duration = Duration::from_secs(60);

/// Model quota of the active profile. It is refreshed only while the panel
/// is visible: when it opens, by timer and after runs.
pub(super) struct QuotaWidget {
    host: Host,
    /// `Some(None)`: the active provider reports no quota.
    loaded: Option<Option<ModelQuotaSnapshot>>,
    error: Option<String>,
    loading: bool,
    _timer: Task<()>,
    _events: Option<Subscription>,
}

pub(super) fn build(host: Host, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let events = host.subscribe(cx, |this: &mut QuotaWidget, event, cx| {
            if event != HostEvent::SessionOpened {
                this.refresh(cx);
            }
        });
        let timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        cx.defer_in(window, |this, _, cx| this.refresh(cx));
        QuotaWidget {
            host,
            loaded: None,
            error: None,
            loading: false,
            _timer: timer,
            _events: events,
        }
    })
    .into()
}

impl QuotaWidget {
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let visible = self.host.read(cx, |host| host.panel_open()) == Some(true);
        if self.loading || !visible {
            return;
        }
        let (Some(api), Some(dir)) = (self.host.api(cx), self.host.session_dir(cx)) else {
            return;
        };
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.model_quota(&dir) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) => {
                        this.loaded = Some(snapshot);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn body(&self, cx: &Context<Self>) -> AnyElement {
        let snapshot = self.loaded.as_ref().and_then(Option::as_ref);
        let theme = cx.theme();
        let content = match &self.loaded {
            None if self.error.is_none() => muted("Загрузка…", cx).into_any_element(),
            None => div().into_any_element(),
            Some(None) => muted("Текущий провайдер не предоставляет сведения о лимитах.", cx)
                .into_any_element(),
            Some(Some(snapshot)) => render_snapshot(snapshot, cx).into_any_element(),
        };
        let status = match (&self.error, snapshot) {
            (Some(error), _) => div()
                .text_xs()
                .text_color(theme.danger)
                .child(format!("Не удалось получить лимиты: {error}")),
            (None, Some(snapshot)) => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("Данные на {}", local_time(snapshot.observed_at))),
            (None, None) => div(),
        };
        v_flex()
            .gap_2()
            .child(content)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(status.flex_1().min_w_0())
                    .child(
                        Button::new("quota-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .tooltip("Обновить")
                            .loading(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for QuotaWidget {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self
            .loaded
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(weekly_remaining)
            .map(|left| format!("осталось {}", percent(left)).into());
        let body = is_open(ID, cx).then(|| self.body(cx));
        widget_frame(ID, "Лимиты модели".into(), summary, body, cx)
    }
}

fn render_snapshot(snapshot: &ModelQuotaSnapshot, cx: &App) -> Div {
    let theme = cx.theme();
    let mut body = v_flex().gap_3();
    if let Some(plan) = &snapshot.plan {
        body = body.child(muted(format!("План · {plan}"), cx));
    }
    if snapshot.buckets.is_empty() {
        body = body.child(muted("Окна лимитов не предоставлены.", cx));
    }
    for (bucket_ix, bucket) in snapshot.buckets.iter().enumerate() {
        let mut section = v_flex()
            .gap_1()
            .child(div().text_sm().font_medium().child(bucket_name(bucket)));
        if bucket.limit_reached == Some(true) {
            section = section.child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child("Лимит исчерпан"),
            );
        } else if bucket.allowed == Some(false) {
            section = section.child(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child("Использование сейчас недоступно"),
            );
        }
        if bucket.windows.is_empty() {
            section = section.child(muted("Окна лимитов не предоставлены.", cx));
        }
        for (window_ix, window) in bucket.windows.iter().enumerate() {
            let left = remaining(window.used_percent);
            let color = if left <= 10. {
                theme.danger
            } else {
                theme.muted_foreground
            };
            section = section
                .child(row(
                    duration(window.duration_seconds, &window.id),
                    format!("осталось {}", percent(left)),
                    cx,
                ))
                .child(
                    Progress::new(("quota-bar", bucket_ix * 16 + window_ix))
                        .value(left as f32)
                        .color(color)
                        .xsmall(),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(reset_label(window.resets_at, now_seconds())),
                );
        }
        body = body.child(section);
    }
    if let Some(credits) = &snapshot.credits {
        let value = if credits.unlimited {
            "Без ограничений".to_owned()
        } else if let Some(balance) = &credits.balance {
            balance.clone()
        } else if credits.available {
            "Доступны".to_owned()
        } else {
            "Нет доступных".to_owned()
        };
        body = body.child(row("Кредиты", value, cx));
    }
    body
}

fn bucket_name(bucket: &ModelQuotaBucket) -> String {
    bucket.name.clone().unwrap_or_else(|| bucket.id.clone())
}

/// Remaining share of the most used weekly window, as in the web widget.
fn weekly_remaining(snapshot: &ModelQuotaSnapshot) -> Option<f64> {
    snapshot
        .buckets
        .iter()
        .flat_map(|bucket| &bucket.windows)
        .filter(|window| window.duration_seconds == Some(WEEK_SECONDS))
        .map(|window| remaining(window.used_percent))
        .min_by(f64::total_cmp)
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn number(value: f64) -> String {
    let rounded = (value * 10.).round() / 10.;
    if rounded.fract() == 0. {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}").replace('.', ",")
    }
}

fn duration(seconds: Option<u64>, fallback: &str) -> String {
    let Some(seconds) = seconds else {
        return fallback.to_owned();
    };
    for (size, label) in [(86_400, "дн."), (3_600, "ч"), (60, "мин")] {
        if seconds > 0 && seconds % size == 0 {
            return format!("{} {label}", seconds / size);
        }
    }
    format!("{seconds} с")
}

fn remaining(used: f64) -> f64 {
    (100. - used).clamp(0., 100.)
}

fn percent(value: f64) -> String {
    format!("{}%", number(value))
}

fn reset_label(resets_at: Option<u64>, now: u64) -> String {
    match resets_at {
        None => "Время сброса не указано".to_owned(),
        Some(at) if at <= now => "Время сброса прошло · ожидаем новые данные".to_owned(),
        Some(at) => format!("Сброс {}", local_time(at)),
    }
}

#[cfg(test)]
mod tests {
    use super::{duration, percent, remaining, reset_label};

    #[test]
    fn formats_quota_windows_like_the_web_widget() {
        assert_eq!(duration(Some(604_800), "w"), "7 дн.");
        assert_eq!(duration(Some(18_000), "w"), "5 ч");
        assert_eq!(duration(Some(90), "w"), "90 с");
        assert_eq!(duration(None, "primary"), "primary");
        assert_eq!(percent(remaining(92.)), "8%");
        assert_eq!(percent(remaining(92.5)), "7,5%");
        assert_eq!(remaining(130.), 0.);
        assert_eq!(reset_label(None, 10), "Время сброса не указано");
        assert_eq!(
            reset_label(Some(5), 10),
            "Время сброса прошло · ожидаем новые данные"
        );
        assert!(reset_label(Some(20), 10).starts_with("Сброс "));
    }
}
