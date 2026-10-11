use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::*;
use gpui_kit::*;
use proteus_contracts::contracts::{ModelQuotaBucket, ModelQuotaSnapshot};

use super::badge::{Details, badge_frame};
use super::host::{Host, HostEvent};
use super::session_badges::ring_face;
use super::shared::shared;
use crate::ui::format::local_time;

pub(super) const ID: &str = "quota";
const WEEK_SECONDS: u64 = 604_800;
const REFRESH: Duration = Duration::from_secs(60);

/// Model quota of the active profile: refreshed by timer and after runs.
struct QuotaModel {
    host: Host,
    /// `Some(None)`: the active provider reports no quota.
    loaded: Option<Option<ModelQuotaSnapshot>>,
    error: Option<String>,
    loading: bool,
    _timer: Task<()>,
    _events: Option<Subscription>,
}

fn model(host: Host, window: &mut Window, cx: &mut App) -> Entity<QuotaModel> {
    shared(cx, |cx| {
        cx.new(|cx| {
            let events = host.subscribe(cx, |this: &mut QuotaModel, event, cx| {
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
            QuotaModel {
                host,
                loaded: None,
                error: None,
                loading: false,
                _timer: timer,
                _events: events,
            }
        })
    })
}

impl QuotaModel {
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
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

    fn snapshot(&self) -> Option<&ModelQuotaSnapshot> {
        self.loaded.as_ref().and_then(Option::as_ref)
    }
}

/// Icon: a ring of the used share of the tightest weekly window; every
/// window, plan and credits are in its tooltip.
struct QuotaBadge {
    host: Host,
    model: Entity<QuotaModel>,
    _observe: Subscription,
}

pub(super) fn build_badge(host: Host, window: &mut Window, cx: &mut App) -> AnyView {
    let model = model(host.clone(), window, cx);
    cx.new(|cx| QuotaBadge {
        host,
        _observe: cx.observe(&model, |_, _, cx| cx.notify()),
        model,
    })
    .into()
}

impl Render for QuotaBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let theme = cx.theme();
        let left = model.snapshot().and_then(weekly_remaining);
        let mut details = Details::new(match left {
            Some(left) => format!("Лимиты: осталось {}", percent(left)),
            None => "Лимиты модели".to_owned(),
        });
        match (&model.error, &model.loaded) {
            (Some(error), _) => details = details.line(format!("Не удалось получить: {error}")),
            (None, None) => details = details.line("Загрузка…"),
            (None, Some(None)) => details = details.line("Провайдер не сообщает лимиты."),
            (None, Some(Some(snapshot))) => details = describe(snapshot, details),
        }
        let face = match left {
            Some(left) => {
                let color = if left <= 10. {
                    theme.danger
                } else if left <= 25. {
                    theme.warning
                } else {
                    theme.muted_foreground
                };
                ring_face("quota-badge-ring", (100. - left) as f32, color)
            }
            None => Rc::new(|| Icon::new(Lucide::Gauge).small().into_any_element()),
        };
        badge_frame(ID, face, details, &self.host, cx)
    }
}

fn describe(snapshot: &ModelQuotaSnapshot, mut details: Details) -> Details {
    if let Some(plan) = &snapshot.plan {
        details = details.line(format!("План: {plan}"));
    }
    for bucket in &snapshot.buckets {
        if bucket.limit_reached == Some(true) {
            details = details.line(format!("{}: лимит исчерпан", bucket_name(bucket)));
        } else if bucket.allowed == Some(false) {
            details = details.line(format!("{}: использование недоступно", bucket_name(bucket)));
        }
        for window in &bucket.windows {
            details = details.line(format!(
                "{} · {}: осталось {} · {}",
                bucket_name(bucket),
                duration(window.duration_seconds, &window.id),
                percent(remaining(window.used_percent)),
                reset_label(window.resets_at, now_seconds()),
            ));
        }
    }
    if let Some(credits) = &snapshot.credits {
        let value = if credits.unlimited {
            "без ограничений".to_owned()
        } else if let Some(balance) = &credits.balance {
            balance.clone()
        } else if credits.available {
            "доступны".to_owned()
        } else {
            "нет доступных".to_owned()
        };
        details = details.line(format!("Кредиты: {value}"));
    }
    details.line(format!("Данные на {}", local_time(snapshot.observed_at)))
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
