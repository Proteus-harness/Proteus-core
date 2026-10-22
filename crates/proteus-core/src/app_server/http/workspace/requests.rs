//! Strict JSON bodies of the `POST /workspace/*` endpoints.
use std::path::PathBuf;

use serde::Deserialize;

use super::{files::EntryKind, git::IgnoreTarget};

/// A body that addresses a live session by its `session_dir`.
pub(super) trait Addressed {
    fn session_dir(&self) -> PathBuf;
}

macro_rules! requests {
    ($(
        $(#[$meta:meta])*
        struct $name:ident { $($(#[$field_meta:meta])* $field:ident: $type:ty),* $(,)? }
    )*) => {$(
        $(#[$meta])*
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(super) struct $name {
            session_dir: PathBuf,
            $($(#[$field_meta])* pub(super) $field: $type,)*
        }

        impl Addressed for $name {
            fn session_dir(&self) -> PathBuf {
                self.session_dir.clone()
            }
        }
    )*};
}

requests! {
    /// Fetch, stash pop and uncommit carry no parameters.
    struct SessionRequest {}
    struct PathsRequest {
        #[serde(default)] paths: Vec<String>,
        #[serde(default)] all: bool,
    }
    struct CommitRequest {
        message: String,
        #[serde(default)] amend: bool,
        #[serde(default)] signoff: bool,
        #[serde(default)] no_verify: bool,
        #[serde(default)] all_tracked: bool,
    }
    struct CheckoutRequest {
        branch: String,
        #[serde(default)] create: bool,
    }
    struct PullRequest {
        #[serde(default)] rebase: Option<bool>,
    }
    struct PushRequest {
        #[serde(default)] force: bool,
        #[serde(default)] set_upstream: bool,
    }
    struct StashRequest {
        #[serde(default)] message: Option<String>,
        #[serde(default)] staged_only: bool,
    }
    struct IgnoreRequest { paths: Vec<String>, target: IgnoreTarget }
    struct CreateRequest { path: String, kind: EntryKind }
    struct RenameRequest { from: String, to: String }
    struct DeleteRequest { paths: Vec<String> }
}
