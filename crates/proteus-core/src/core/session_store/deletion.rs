//! Explicit local-owner deletion; cold path deletion never borrows this capability.
use super::*;

impl SessionStore {
    /// Validate the configured scope before an app-server closes admission.
    pub(crate) fn validate_owned_delete_scope(
        &self,
        config_root: &Path,
        workspace: &Path,
    ) -> Result<()> {
        let name = self
            .session_dir
            .file_name()
            .context("invalid session directory")?;
        let expected = canonicalize_unmaterialized_path(
            &config_root
                .join("sessions")
                .join(encode_workspace_path(workspace)?)
                .join(name),
        )?;
        anyhow::ensure!(
            expected == self.session_dir,
            "session path is outside configured workspace sessions"
        );
        Ok(())
    }

    /// Only an explicit runtime's store may transfer its local generation.
    /// The caller must first settle its admitted work and close admission.
    pub(crate) async fn delete_owned(&self, config_root: &Path, workspace: &Path) -> Result<bool> {
        self.delete_owned_with(config_root, workspace, || Ok(()))
            .await
    }

    async fn delete_owned_with(
        &self,
        config_root: &Path,
        workspace: &Path,
        before_remove: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> Result<bool> {
        self.validate_owned_delete_scope(config_root, workspace)?;
        let mut writer = self.writer.clone().lock_owned().await;
        let store = self.clone();
        let config_root = config_root.to_owned();
        let workspace = workspace.to_owned();
        tokio::task::spawn_blocking(move || {
            writer.ensure_active()?;
            store.validate_owned_delete_scope(&config_root, &workspace)?;
            let parent = store
                .session_dir
                .parent()
                .context("session has no parent")?;
            // An unsaved session may have no parent. Retire it without creating
            // directories or a lease for a slot that will not be removed.
            let parent_exists = parent.try_exists()?;
            let exists = if parent_exists {
                writer.with_ownership_preflight(&store.session_dir, store.session_id, |_| {
                    match std::fs::symlink_metadata(&store.session_dir) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                        Err(error) => Err(error.into()),
                        Ok(metadata) => {
                            anyhow::ensure!(
                                metadata.is_dir() && !metadata.file_type().is_symlink(),
                                "session path is not a directory: {}",
                                store.session_dir.display()
                            );
                            let identity = resolve_session_identity(&store.session_dir)?;
                            anyhow::ensure!(
                                identity.session_id == store.session_id,
                                "session identity changed before owned deletion"
                            );
                            Ok(true)
                        }
                    }
                })?
            } else {
                false
            };
            // Move ownership before releasing the shared mutex. Retired handles
            // now fail immediately; the independent worker retains the OS lease.
            let tail = writer.retire_for_delete()?;
            drop(writer);
            before_remove()?;
            if exists {
                std::fs::remove_dir_all(&store.session_dir)
                    .with_context(|| format!("failed to delete {}", store.session_dir.display()))?;
            }
            // Completion is published only after the actual OS lease has closed.
            drop(tail);
            Ok(exists)
        })
        .await
        .context("owned session deletion worker failed")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{new_session_id, new_thread_id};
    use std::fs::{OpenOptions, TryLockError};
    use std::time::Duration;

    #[tokio::test]
    async fn canceled_owned_delete_retains_lease_and_retires_clones() {
        let config = tempfile::tempdir().expect("config");
        let workspace = tempfile::tempdir().expect("workspace");
        let store =
            SessionStore::new(config.path(), workspace.path(), new_session_id()).expect("store");
        let thread = new_thread_id();
        let message = CanonicalMessage::text(MessageRole::User, "owned deletion");
        store
            .append_history(thread, None, &[message.clone()])
            .await
            .expect("materialize");
        let stale = store.clone();
        let session_dir = store.session_dir().to_owned();
        let lock_path = session_dir.parent().unwrap().join(format!(
            ".{}.write.lock",
            session_dir.file_name().unwrap().to_str().unwrap()
        ));
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .expect("lease");
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        // Dropping this on panic also unblocks the worker. Its own finite wait
        // prevents a failed control from hanging the full suite.
        struct Release(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }
        let release = Release(Some(release));
        let config_path = config.path().to_owned();
        let workspace_path = workspace.path().to_owned();
        let deleting = tokio::spawn(async move {
            store
                .delete_owned_with(&config_path, &workspace_path, move || {
                    let _ = entered.send(());
                    resume
                        .recv_timeout(Duration::from_secs(10))
                        .context("bounded delete pause")?;
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), ready)
            .await
            .expect("delete reaches worker")
            .expect("worker ready");
        assert!(session_dir.exists(), "pause precedes removal");
        assert!(matches!(lease.try_lock(), Err(TryLockError::WouldBlock)));
        deleting.abort();
        assert!(deleting.await.expect_err("waiter canceled").is_cancelled());
        assert!(
            matches!(lease.try_lock(), Err(TryLockError::WouldBlock)),
            "OWNERSHIP_CANCELED_DELETE: canceled waiter must not release actual lease"
        );
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            stale.append_history(thread, None, &[message.clone()]),
        )
        .await
        .expect("retired append does not wait for blocking removal")
        .expect_err("retired clone cannot append");
        assert!(error.to_string().contains("retired"), "{error:#}");
        drop(release);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match lease.try_lock() {
                    Ok(()) => break,
                    Err(TryLockError::WouldBlock) => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    Err(error) => panic!("lease probe: {error}"),
                }
            }
        })
        .await
        .expect("lease closes after worker completion");
        assert!(
            !session_dir.exists(),
            "lease cannot become free before removal finishes"
        );
        drop(lease); // Do not let the probe's competing lock mask retirement as Busy.
        let error = stale
            .append_history(thread, None, &[message])
            .await
            .expect_err("retired clone stays retired after completion");
        assert!(error.to_string().contains("retired"), "{error:#}");
        assert!(
            !session_dir.exists(),
            "retired clone never recreates deleted storage"
        );
    }

    #[tokio::test]
    async fn rejected_owned_delete_preserves_committed_owner() {
        let config = tempfile::tempdir().expect("config");
        let other = tempfile::tempdir().expect("outside");
        let workspace = tempfile::tempdir().expect("workspace");
        let store =
            SessionStore::new(config.path(), workspace.path(), new_session_id()).expect("store");
        let thread = new_thread_id();
        let message = CanonicalMessage::text(MessageRole::User, "valid owner");
        store
            .append_history(thread, None, &[message.clone()])
            .await
            .expect("materialize");
        store
            .delete_owned(other.path(), workspace.path())
            .await
            .expect_err("scope mismatch");
        let following = CanonicalMessage::text(MessageRole::Assistant, "owner remains live");
        store
            .append_history(thread, None, &[following])
            .await
            .expect("owner still live");
        super::super::delete_workspace_session(
            config.path(),
            workspace.path(),
            store.session_dir().to_owned(),
        )
        .await
        .expect_err("committed lease remains held");
    }
}
