//! Parsed assistant replies, kept for the chat's lifetime: a reply scrolled
//! out of view or left in a background tab comes back as it was, without a
//! fresh parse and an empty first frame that makes the thread jump.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::text::TextViewState;
use gpui_kit::*;

use super::Chat;

/// gpui-base parses a full text longer than this in the background, so its
/// first frame is nearly empty until the parse lands.
const BACKGROUND_PARSE_BYTES: usize = 4 * 1024;

#[derive(Default)]
pub(super) struct Replies(HashMap<u64, Reply>);

struct Reply {
    state: Entity<TextViewState>,
    /// The text the state was last given; a new `Arc` means new text.
    text: Arc<str>,
    _changes: Subscription,
}

impl Replies {
    /// The parsed reply of row `key`, brought up to `text`.
    pub(super) fn state(
        &mut self,
        key: u64,
        text: &Arc<str>,
        cx: &mut Context<Chat>,
    ) -> Entity<TextViewState> {
        if let Some(reply) = self.0.get_mut(&key) {
            if !Arc::ptr_eq(&reply.text, text) {
                reply.text = text.clone();
                reply.state.update(cx, |state, cx| state.set_text(text, cx));
            }
            return reply.state.clone();
        }
        let state = cx.new(|cx| TextViewState::markdown(text, cx));
        // Parsing finishes in the background and streamed text fades in; the
        // row is measured again, or a row out of view keeps its old height
        // and jumps once scrolled to.
        let changes = cx.observe(&state, move |chat, _, cx| chat.remeasure_row(key, cx));
        self.0.insert(
            key,
            Reply {
                state: state.clone(),
                text: text.clone(),
                _changes: changes,
            },
        );
        state
    }

    /// Starts parsing the long replies of a loaded thread ahead of their
    /// rows, so scrolling up meets them at full height.
    pub(super) fn prepare<'a>(
        &mut self,
        replies: impl IntoIterator<Item = (u64, &'a Arc<str>)>,
        cx: &mut Context<Chat>,
    ) {
        for (key, text) in replies {
            if text.len() > BACKGROUND_PARSE_BYTES && !self.0.contains_key(&key) {
                self.state(key, text, cx);
            }
        }
    }

    /// Forgets replies whose rows left the thread.
    pub(super) fn retain(&mut self, keys: &HashSet<u64>) {
        self.0.retain(|key, _| keys.contains(key));
    }
}
