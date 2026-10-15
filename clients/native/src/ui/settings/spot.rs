//! Following a search hit: its page opens with the way to the row
//! unfolded, the row scrolls into view and is briefly marked.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::InputState;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::search::Hit;
use crate::settings::FieldPath;
use crate::settings::catalog::PageId;
use crate::ui::motion;

/// The mark stays full for a moment, then fades.
const HOLD: Duration = Duration::from_millis(700);
const FADE: Duration = Duration::from_millis(900);
const PEAK: f32 = 0.16;
/// Frames in which the page may still move to the row while its layout
/// settles.
const SETTLE_FRAMES: u8 = 4;
/// Room kept above a row scrolled into view.
const MARGIN: f32 = 48.;

pub(super) struct Spot {
    key: String,
    since: Instant,
    settle: Rc<Cell<u8>>,
}

pub(super) fn field(path: &FieldPath) -> String {
    format!("field:{}:{}:{}", path.slot, path.module, path.dotted())
}

pub(super) fn row(title: &str) -> String {
    format!("row:{title}")
}

pub(super) fn provider(id: &str) -> String {
    format!("provider:{id}")
}

pub(super) fn module(slot: &str, id: &str) -> String {
    format!("module:{slot}:{id}")
}

pub(super) fn export(slot: &str, id: &str) -> String {
    format!("export:{slot}:{id}")
}

pub(super) fn tool(name: &str) -> String {
    format!("tool:{name}")
}

pub(super) fn hook(id: &str) -> String {
    format!("hook:{id}")
}

pub(super) fn component(id: &str) -> String {
    format!("component:{id}")
}

pub(super) fn skill(id: &str) -> String {
    format!("skill:{id}")
}

pub(super) fn mcp(server: &str) -> String {
    format!("mcp:{server}")
}

pub(super) fn package(path: &std::path::Path) -> String {
    format!("package:{}", path.display())
}

/// Strength of the mark `elapsed` after the jump; without animations it
/// stays full and then disappears.
fn strength(elapsed: Duration) -> f32 {
    if elapsed < HOLD {
        return PEAK;
    }
    if elapsed >= HOLD + FADE {
        return 0.;
    }
    if !motion::plays() {
        return PEAK;
    }
    let t = (elapsed - HOLD).as_secs_f32() / FADE.as_secs_f32();
    PEAK * (1. - motion::ease_exit(t))
}

impl SettingsView {
    pub(super) fn go_to(&mut self, hit: &Hit, window: &mut Window, cx: &mut Context<Self>) {
        // Unfolded without animation, so the row has its place at once.
        self.open.extend(hit.open.iter().cloned());
        if hit.page == PageId::Tools && !self.tool_query.is_empty() {
            self.tool_query.clear();
            self.tool_search.update(cx, |state: &mut InputState, cx| {
                state.set_value("", window, cx)
            });
        }
        self.select_page(hit.page.clone(), window, cx);
        self.spot = hit.spot.clone().map(|key| Spot {
            key,
            since: Instant::now(),
            settle: Rc::new(Cell::new(SETTLE_FRAMES)),
        });
    }

    /// The scroll position of the shown page.
    pub(super) fn page_scroll(&self) -> ScrollHandle {
        self.scrolls
            .get(&page_key(&self.page))
            .cloned()
            .unwrap_or_default()
    }

    /// `element`, marked and kept in view while it is the found row.
    pub(super) fn spotted(&self, key: &str, element: impl IntoElement, cx: &App) -> AnyElement {
        let Some(spot) = self.spot.as_ref().filter(|spot| spot.key == key) else {
            return element.into_any_element();
        };
        let elapsed = spot.since.elapsed();
        let strength = strength(elapsed);
        let running = elapsed < HOLD + FADE;
        let (scroll, settle) = (self.page_scroll(), spot.settle.clone());
        let follow = canvas(
            move |bounds, window, _| {
                bring_into_view(&scroll, bounds, &settle);
                if running || settle.get() > 0 {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let theme = cx.theme();
        v_flex()
            .relative()
            .child(element)
            .child(follow)
            .children((strength > 0.).then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .rounded(theme.radius)
                    .bg(theme.primary.opacity(strength))
            }))
            .into_any_element()
    }
}

pub(super) fn page_key(page: &PageId) -> String {
    format!("{page:?}")
}

/// Moves the page so `target` is inside it with some room above, for the
/// few frames the layout may still change after the jump.
fn bring_into_view(scroll: &ScrollHandle, target: Bounds<Pixels>, settle: &Cell<u8>) {
    let left = settle.get();
    if left == 0 {
        return;
    }
    settle.set(left - 1);
    let view = scroll.bounds();
    if view.size.height <= px(0.) {
        return;
    }
    let margin = px(MARGIN).min(view.size.height / 4.);
    let shift = if target.top() < view.top() + margin {
        view.top() + margin - target.top()
    } else if target.bottom() > view.bottom() - margin {
        // A row taller than the page shows its top.
        (view.bottom() - margin - target.bottom()).max(view.top() + margin - target.top())
    } else {
        return;
    };
    let offset = scroll.offset();
    let lowest = -scroll.max_offset().y;
    let y = (offset.y + shift).clamp(lowest, px(0.));
    if y != offset.y {
        scroll.set_offset(point(offset.x, y));
    }
}
