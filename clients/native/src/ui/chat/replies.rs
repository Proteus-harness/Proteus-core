//! Parsed assistant replies, kept for the chat's lifetime: a reply scrolled
//! out of view or left in a background tab comes back as it was, without a
//! fresh parse and an empty first frame that makes the thread jump.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::text::TextViewState;
use gpui_kit::*;

use super::Chat;

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
        // Parsing finishes in the background and streamed text fades in.
        let changes = cx.observe(&state, |_, _, cx| cx.notify());
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

    /// Forgets replies whose rows left the thread.
    pub(super) fn retain(&mut self, keys: &HashSet<u64>) {
        self.0.retain(|key, _| keys.contains(key));
    }
}
