use std::{borrow::Cow, collections::HashSet};

mod compacted_user;

use anyhow::{Result, ensure};

use crate::{
    domain::{HistoryCompactionReport, MessageId, TurnId},
    model_standard::{CanonicalMessage, MessageRole},
};

#[derive(Debug)]
pub(crate) struct PreparedHistoryUpdate {
    pub(crate) final_messages: Vec<CanonicalMessage>,
    pub(crate) replace: bool,
}

pub(crate) fn prepare_history_update(
    current_history: &[CanonicalMessage],
    persisted_user_message: &CanonicalMessage,
    new_messages: &[CanonicalMessage],
    history_replacement: Option<&[CanonicalMessage]>,
    compactions: &[HistoryCompactionReport],
    runtime_user_messages: &HashSet<MessageId>,
) -> Result<PreparedHistoryUpdate> {
    prepare_successful_update(
        Cow::Borrowed(current_history),
        persisted_user_message,
        new_messages,
        history_replacement,
        compactions,
        runtime_user_messages,
    )
}

pub(super) fn prepare_owned_history_update(
    current_history: Vec<CanonicalMessage>,
    persisted_user_message: &CanonicalMessage,
    new_messages: &[CanonicalMessage],
    history_replacement: Option<&[CanonicalMessage]>,
    compactions: &[HistoryCompactionReport],
    runtime_user_messages: &HashSet<MessageId>,
) -> Result<PreparedHistoryUpdate> {
    prepare_successful_update(
        Cow::Owned(current_history),
        persisted_user_message,
        new_messages,
        history_replacement,
        compactions,
        runtime_user_messages,
    )
}

fn prepare_successful_update(
    current_history: Cow<'_, [CanonicalMessage]>,
    persisted_user_message: &CanonicalMessage,
    new_messages: &[CanonicalMessage],
    history_replacement: Option<&[CanonicalMessage]>,
    compactions: &[HistoryCompactionReport],
    runtime_user_messages: &HashSet<MessageId>,
) -> Result<PreparedHistoryUpdate> {
    ensure!(
        !new_messages.is_empty(),
        "workflow returned no new persistent turn messages"
    );
    prepare_update(
        current_history,
        persisted_user_message,
        new_messages,
        history_replacement,
        compactions,
        runtime_user_messages,
    )
}

/// A failed workflow can return a completed replacement without a later answer.
pub(crate) fn prepare_failed_history_update(
    current_history: &[CanonicalMessage],
    persisted_user_message: &CanonicalMessage,
    new_messages: &[CanonicalMessage],
    history_replacement: Option<&[CanonicalMessage]>,
    compactions: &[HistoryCompactionReport],
    runtime_user_messages: &HashSet<MessageId>,
) -> Result<PreparedHistoryUpdate> {
    ensure!(
        !new_messages.is_empty() || history_replacement.is_some(),
        "workflow failure returned an empty history update"
    );
    prepare_update(
        Cow::Borrowed(current_history),
        persisted_user_message,
        new_messages,
        history_replacement,
        compactions,
        runtime_user_messages,
    )
}

fn prepare_update(
    current_history: Cow<'_, [CanonicalMessage]>,
    persisted_user_message: &CanonicalMessage,
    new_messages: &[CanonicalMessage],
    history_replacement: Option<&[CanonicalMessage]>,
    compactions: &[HistoryCompactionReport],
    runtime_user_messages: &HashSet<MessageId>,
) -> Result<PreparedHistoryUpdate> {
    ensure!(
        current_history.last() == Some(persisted_user_message),
        "runtime history does not end with the persisted current user message"
    );
    for (index, message) in new_messages.iter().enumerate() {
        ensure!(
            matches!(message.role, MessageRole::Assistant | MessageRole::Tool)
                || (message.role == MessageRole::User
                    && runtime_user_messages.contains(&message.id)),
            "workflow new_messages[{index}] has non-persistent turn role {:?}",
            message.role
        );
    }

    match (
        compactions.iter().any(|report| report.changed),
        history_replacement,
    ) {
        (true, None) => {
            anyhow::bail!("workflow reported changed compaction without history replacement")
        }
        (false, Some(_)) => {
            anyhow::bail!("workflow returned history replacement without changed compaction")
        }
        (true, Some(replacement)) => {
            compacted_user::validate_current_user(
                current_history.as_ref(),
                persisted_user_message,
                replacement,
                compactions,
            )?;
            let mut final_messages = Vec::with_capacity(replacement.len() + new_messages.len());
            final_messages.extend_from_slice(replacement);
            final_messages.extend_from_slice(new_messages);
            Ok(PreparedHistoryUpdate {
                final_messages,
                replace: true,
            })
        }
        (false, None) => {
            let mut final_messages = match current_history {
                Cow::Borrowed(prefix) => {
                    let mut messages = Vec::with_capacity(prefix.len() + new_messages.len());
                    messages.extend_from_slice(prefix);
                    messages
                }
                Cow::Owned(mut prefix) => {
                    prefix.reserve_exact(new_messages.len());
                    prefix
                }
            };
            final_messages.extend_from_slice(new_messages);
            Ok(PreparedHistoryUpdate {
                final_messages,
                replace: false,
            })
        }
    }
}

impl super::AgentRuntime {
    pub(super) async fn commit_history_update(
        &self,
        turn_id: TurnId,
        update: PreparedHistoryUpdate,
        compactions: &[HistoryCompactionReport],
        preserve_completed_suffix: bool,
    ) -> Result<()> {
        let mut history = self.session.history.lock().await;
        let mut pending_compaction = compactions
            .iter()
            .rev()
            .find(|report| report.changed)
            .cloned();
        if let Some(store) = &self.session.session_store {
            // A callback can lose its acknowledgement after its journal append.
            let (committed_history, recorded_compactions) = store
                .committed_history(self.session.thread_id, turn_id)
                .await?;
            let offered_compactions = compactions.iter().filter(|report| report.changed).count();
            if recorded_compactions > offered_compactions {
                refresh_committed_history(&mut history, committed_history)?;
                ensure!(
                    preserve_completed_suffix,
                    "successful workflow discarded a committed preparation checkpoint"
                );
                return Ok(());
            }
            pending_compaction = compactions
                .iter()
                .filter(|report| report.changed)
                .skip(recorded_compactions)
                .last()
                .cloned();
            refresh_committed_history(&mut history, committed_history)?;
        }
        if history.starts_with(&update.final_messages) {
            ensure!(
                preserve_completed_suffix || *history == update.final_messages,
                "successful workflow omitted committed checkpoint progress"
            );
            if let Some(store) = &self.session.session_store
                && pending_compaction.is_some()
            {
                store
                    .replace_history(
                        self.session.thread_id,
                        Some(turn_id),
                        &history,
                        pending_compaction,
                    )
                    .await?;
                self.session.model_context.lock().await.compacted();
            }
            if compactions.iter().any(|report| report.changed) {
                self.session.interrupted_turns.lock().await.clear();
            }
            return Ok(());
        }
        let suffix = update.final_messages.strip_prefix(history.as_slice());
        ensure!(
            suffix.is_some() || update.replace,
            "workflow terminal history disagrees with committed checkpoints"
        );
        if let Some(store) = &self.session.session_store {
            if suffix.is_none() || pending_compaction.is_some() {
                store
                    .replace_history(
                        self.session.thread_id,
                        Some(turn_id),
                        &update.final_messages,
                        pending_compaction.clone(),
                    )
                    .await?;
            } else {
                store
                    .append_history(
                        self.session.thread_id,
                        Some(turn_id),
                        suffix.expect("validated history suffix"),
                    )
                    .await?;
            }
        }
        if suffix.is_none() || pending_compaction.is_some() {
            self.session.model_context.lock().await.compacted();
        }
        if compactions.iter().any(|report| report.changed) {
            self.session.interrupted_turns.lock().await.clear();
        }
        if *history != update.final_messages {
            self.session
                .history_revision
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        *history = update.final_messages;
        Ok(())
    }
}

/// Reload missing committed progress without replacing still-live values with
/// their redacted disk representation. Changed checkpoint snapshots still win.
pub(super) fn refresh_committed_history(
    history: &mut Vec<CanonicalMessage>,
    durable: Vec<CanonicalMessage>,
) -> Result<()> {
    let redacted = crate::core::session_journal::redacted_history(history)?;
    let originals = history
        .iter()
        .zip(&redacted)
        .map(|(raw, stored)| (raw.id, (raw, stored)))
        .collect::<std::collections::HashMap<_, _>>();
    let count = history.len();
    // An unchanged prefix already owns exactly the live rows the merge would
    // clone. A repeated ID can select a different live row, even when its
    // redacted form matches, so keep the general merge for those cases.
    if count != 0
        && originals.len() == count
        && let Some(added) = durable.get(count..)
        && history
            .iter()
            .zip(&redacted)
            .zip(&durable)
            .all(|((raw, stored), committed)| raw.id == stored.id && stored == committed)
        && added.iter().all(|row| !originals.contains_key(&row.id))
    {
        if !added.is_empty() {
            drop(originals);
            drop(redacted);
            history.extend(durable.into_iter().skip(count));
        }
        return Ok(());
    }
    let merged = durable
        .into_iter()
        .map(|message| match originals.get(&message.id) {
            Some((raw, stored)) if **stored == message => (*raw).clone(),
            _ => message,
        })
        .collect();
    *history = merged;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshing_unchanged_durable_history_keeps_the_live_buffer() {
        let live = CanonicalMessage::text(MessageRole::Assistant, "known")
            .with_metadata(serde_json::json!({"secret": "live-only"}));
        let mut history = vec![live.clone()];
        let durable = crate::core::session_journal::redacted_history(&history).unwrap();
        assert_ne!(history, durable);
        let allocation = history.as_ptr();

        refresh_committed_history(&mut history, durable).unwrap();

        assert_eq!(history, vec![live]);
        assert_eq!(history.as_ptr(), allocation);
    }

    #[test]
    fn refreshing_a_checkpoint_keeps_live_values_and_adopts_missing_progress() {
        let mut live = CanonicalMessage::text(MessageRole::Assistant, "known");
        live.metadata = serde_json::json!({"secret": "live-only"});
        let mut history = Vec::with_capacity(2);
        history.push(live.clone());
        let live_secret = history[0].metadata["secret"].as_str().unwrap().as_ptr();
        let mut durable = crate::core::session_journal::redacted_history(&history).unwrap();
        let completed = CanonicalMessage::text(MessageRole::Assistant, "acknowledgement lost")
            .with_metadata(serde_json::json!({"note": "new durable value"}));
        durable.push(completed.clone());
        let stored_note = durable[1].metadata["note"].as_str().unwrap().as_ptr();
        refresh_committed_history(&mut history, durable).unwrap();
        assert_eq!(history, vec![live, completed]);
        assert_eq!(
            history[0].metadata["secret"].as_str().unwrap().as_ptr(),
            live_secret
        );
        assert_eq!(
            history[1].metadata["note"].as_str().unwrap().as_ptr(),
            stored_note
        );

        // Same ordered IDs and count do not prove equal content.
        let mut durable = crate::core::session_journal::redacted_history(&history).unwrap();
        durable[0].metadata = serde_json::json!({"checkpoint": "changed"});
        refresh_committed_history(&mut history, durable.clone()).unwrap();
        assert_eq!(history, durable);

        let mut replacement = history[0].clone();
        replacement.metadata = serde_json::json!({"checkpoint": "shorter"});
        refresh_committed_history(&mut history, vec![replacement.clone()]).unwrap();
        assert_eq!(history, vec![replacement]);
    }

    #[test]
    fn refreshing_with_a_repeated_id_uses_the_same_last_live_row() {
        let first = CanonicalMessage::text(MessageRole::Assistant, "same text")
            .with_metadata(serde_json::json!({"secret": "first raw value"}));
        let last = first
            .clone()
            .with_metadata(serde_json::json!({"secret": "last raw value"}));
        let mut history = vec![first, last.clone()];
        let durable = crate::core::session_journal::redacted_history(&history).unwrap();
        assert_eq!(durable[0], durable[1]);
        refresh_committed_history(&mut history, durable).unwrap();
        assert_eq!(history, vec![last.clone(), last]);

        let live = history.pop().unwrap();
        let mut durable = crate::core::session_journal::redacted_history(&history).unwrap();
        durable.push(durable[0].clone());
        refresh_committed_history(&mut history, durable).unwrap();
        assert_eq!(history, vec![live.clone(), live]);
    }

    fn changed_report() -> HistoryCompactionReport {
        let mut report = HistoryCompactionReport::unchanged(1, None);
        report.changed = true;
        report
    }

    #[test]
    fn append_update_keeps_persisted_user_and_adds_turn_messages() {
        let user = CanonicalMessage::text(MessageRole::User, "question");
        let assistant = CanonicalMessage::text(MessageRole::Assistant, "answer");

        let update = prepare_history_update(
            std::slice::from_ref(&user),
            &user,
            std::slice::from_ref(&assistant),
            None,
            &[],
            &HashSet::new(),
        )
        .expect("append update");

        assert!(!update.replace);
        assert_eq!(update.final_messages, vec![user, assistant]);
    }

    #[test]
    fn replacement_must_preserve_exact_persisted_user() {
        let user = CanonicalMessage::text(MessageRole::User, "question");
        let recreated_user = CanonicalMessage::text(MessageRole::User, "question");
        let assistant = CanonicalMessage::text(MessageRole::Assistant, "answer");

        let error = prepare_history_update(
            std::slice::from_ref(&user),
            &user,
            std::slice::from_ref(&assistant),
            Some(std::slice::from_ref(&recreated_user)),
            &[changed_report()],
            &HashSet::new(),
        )
        .expect_err("replacement must preserve the stored message id");

        assert!(error.to_string().contains("exact current user message"));
    }

    #[test]
    fn replacement_can_keep_generated_summary_after_current_user() {
        let user = CanonicalMessage::text(MessageRole::User, "question");
        let summary = CanonicalMessage::text(MessageRole::User, "compacted summary");
        let assistant = CanonicalMessage::text(MessageRole::Assistant, "answer");
        let replacement = vec![user.clone(), summary.clone()];

        let update = prepare_history_update(
            std::slice::from_ref(&user),
            &user,
            std::slice::from_ref(&assistant),
            Some(&replacement),
            &[changed_report()],
            &HashSet::new(),
        )
        .expect("compacted history update");

        assert!(update.replace);
        assert_eq!(update.final_messages, vec![user, summary, assistant]);
    }

    #[test]
    fn new_messages_reject_repeated_user_prompt() {
        let user = CanonicalMessage::text(MessageRole::User, "question");

        let error = prepare_history_update(
            std::slice::from_ref(&user),
            &user,
            std::slice::from_ref(&user),
            None,
            &[],
            &HashSet::new(),
        )
        .expect_err("workflow must return only assistant/tool messages");

        assert!(error.to_string().contains("non-persistent turn role User"));
    }

    #[test]
    fn failed_replacement_without_an_answer_preserves_the_same_validation_boundary() {
        let user = CanonicalMessage::text(MessageRole::User, "question");
        let summary = CanonicalMessage::text(MessageRole::User, "completed summary");
        let replacement = vec![user.clone(), summary];
        let allowed = HashSet::new();
        let original = std::slice::from_ref(&user);
        let update = prepare_failed_history_update(
            original,
            &user,
            &[],
            Some(&replacement),
            &[changed_report()],
            &allowed,
        )
        .unwrap();
        assert_eq!(update.final_messages, replacement);
        assert!(update.replace);
        assert!(
            prepare_history_update(
                original,
                &user,
                &[],
                Some(&replacement),
                &[changed_report()],
                &allowed
            )
            .is_err()
        );
        assert!(
            prepare_failed_history_update(original, &user, &[], Some(&replacement), &[], &allowed)
                .is_err()
        );
        assert!(prepare_failed_history_update(original, &user, &[], None, &[], &allowed).is_err());
        let forged_user = CanonicalMessage::text(MessageRole::User, "injected");
        assert!(
            prepare_failed_history_update(original, &user, &[forged_user], None, &[], &allowed)
                .is_err()
        );
    }
}
