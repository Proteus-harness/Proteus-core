use std::collections::HashMap;

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use proteus_contracts::app_protocol::AppPendingRequests;
use proteus_contracts::contracts::{UserInputAnswer, UserInputRequest, UserInputResponse};

/// Answers being composed for pending `request_user_input` calls.
#[derive(Default)]
pub(super) struct UserInputDrafts {
    selected: HashMap<String, HashMap<String, Vec<String>>>,
    texts: HashMap<(String, String), Entity<InputState>>,
}

impl UserInputDrafts {
    /// Drops drafts of requests the server no longer waits for.
    pub fn retain(&mut self, pending: &AppPendingRequests) {
        let live = |id: &str| {
            pending
                .user_inputs
                .iter()
                .any(|request| request.request_id == id)
        };
        self.selected.retain(|request, _| live(request));
        self.texts.retain(|(request, _), _| live(request));
    }

    pub fn is_selected(&self, request: &str, question: &str, label: &str) -> bool {
        self.selected
            .get(request)
            .and_then(|answers| answers.get(question))
            .is_some_and(|labels| labels.iter().any(|selected| selected == label))
    }

    pub fn toggle(&mut self, request: &str, question: &str, label: &str, multi_select: bool) {
        let labels = self
            .selected
            .entry(request.to_owned())
            .or_default()
            .entry(question.to_owned())
            .or_default();
        match labels.iter().position(|selected| selected == label) {
            Some(index) => {
                labels.remove(index);
            }
            None if multi_select => labels.push(label.to_owned()),
            None => *labels = vec![label.to_owned()],
        }
    }

    pub fn text_input(
        &mut self,
        request: &str,
        question: &str,
        secret: bool,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<InputState> {
        self.texts
            .entry((request.to_owned(), question.to_owned()))
            .or_insert_with(|| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .masked(secret)
                })
            })
            .clone()
    }

    /// Every question needs a selected option or typed text.
    pub fn response(&self, request: &UserInputRequest, cx: &App) -> Option<UserInputResponse> {
        let mut answers = HashMap::new();
        for question in &request.questions {
            let mut values = self
                .selected
                .get(&request.request_id)
                .and_then(|answers| answers.get(&question.id))
                .cloned()
                .unwrap_or_default();
            if let Some(input) = self
                .texts
                .get(&(request.request_id.clone(), question.id.clone()))
            {
                let text = input.read(cx).value().trim().to_owned();
                if !text.is_empty() {
                    values.push(text);
                }
            }
            if values.is_empty() {
                return None;
            }
            answers.insert(question.id.clone(), UserInputAnswer::new(values));
        }
        Some(UserInputResponse::new(answers))
    }
}
