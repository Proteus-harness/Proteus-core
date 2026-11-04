//! Prepare user input once and reserve the steering state without making an
//! app-server admission mutex wait for terminal publication.
use super::{
    AgentRuntime, ExecutionAdmissionSnapshot, UserMessageReservation, UserMessageReservationAttempt,
};
use crate::{
    domain::{Event, EventContext},
    model_standard::CanonicalMessage,
};
use anyhow::Result;

impl AgentRuntime {
    /// Atomically reserves an idle root session or appends to its bounded
    /// steering queue. App-server transports call this before spawning a turn,
    /// eliminating the race between the first and second `Send` commands.
    #[cfg(test)]
    pub(crate) async fn reserve_user_message(
        &self,
        text: String,
    ) -> Result<UserMessageReservation> {
        self.reserve_user_message_with_options(text, crate::domain::RunOptions::default())
            .await
    }

    pub(crate) async fn reserve_user_message_with_options(
        &self,
        input: impl Into<crate::domain::UserMessageInput>,
        options: crate::domain::RunOptions,
    ) -> Result<UserMessageReservation> {
        let message = self.prepare_user_message(input, &options).await?;
        let snapshot = self.capture_run_snapshot(options.permission_mode).await;
        let reservation = self
            .session
            .steering
            .reserve_message_with_options(message, options)
            .await?;
        self.bind_user_message_reservation(reservation, snapshot)
            .await
    }

    pub(crate) async fn prepare_user_message(
        &self,
        input: impl Into<crate::domain::UserMessageInput>,
        options: &crate::domain::RunOptions,
    ) -> Result<CanonicalMessage> {
        if let Some(intent) = &options.intent {
            anyhow::ensure!(
                !intent.is_empty()
                    && intent.len() <= 128
                    && intent
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c)),
                "invalid workflow intent name"
            );
        }
        self.validate_cached_resume().await?;
        let input = input.into();
        let input = tokio::task::spawn_blocking(move || super::images::ImageStore::prepare(input))
            .await??;
        if input.has_images()
            && let Some(store) = &self.session.session_store
        {
            store.prepare_attachments().await?;
        }
        let images = self.services.images.clone();
        tokio::task::spawn_blocking(move || images.admit(input)).await?
    }

    pub(crate) async fn try_reserve_prepared_user_message(
        &self,
        message: CanonicalMessage,
        options: crate::domain::RunOptions,
    ) -> Result<UserMessageReservationAttempt> {
        let snapshot = self.capture_run_snapshot(options.permission_mode).await;
        match self
            .session
            .steering
            .try_reserve_message_with_options(message, options)?
        {
            UserMessageReservationAttempt::Ready(reservation) => self
                .bind_user_message_reservation(reservation, snapshot)
                .await
                .map(UserMessageReservationAttempt::Ready),
            deferred => Ok(deferred),
        }
    }

    pub(crate) async fn wait_user_message_finalization(&self) {
        self.session.steering.wait_finalization().await;
    }

    #[cfg(test)]
    pub(crate) async fn wait_deferred_admission_for_test(&self) {
        self.session.steering.wait_deferred_admission().await;
    }

    async fn bind_user_message_reservation(
        &self,
        mut reservation: UserMessageReservation,
        snapshot: ExecutionAdmissionSnapshot,
    ) -> Result<UserMessageReservation> {
        if let UserMessageReservation::Start(reserved) = &mut reservation {
            reserved.snapshot = Some(snapshot);
        }
        if let UserMessageReservation::Queued(receipt) = &reservation {
            self.services
                .events
                .emit(
                    EventContext::new(
                        self.session.session_id,
                        self.session.thread_id,
                        Some(receipt.active_turn_id),
                    ),
                    Event::SteeringQueued {
                        message_id: receipt.message_id,
                        text: receipt.text.clone(),
                        queued_count: receipt.queued_count,
                    },
                )
                .await?;
        }
        Ok(reservation)
    }
}
