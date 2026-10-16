use std::path::PathBuf;

use gpui_kit::*;
use proteus_contracts::app_protocol::AppSessionActivity;

use crate::api::Api;
use crate::ui::chat::{Chat, Session};
use crate::ui::workspace::Workspace;

/// What an extension gets from the client: read access to the active chat's
/// session,
/// the server API, host events and navigation. It holds no strong reference,
/// so extensions never keep the workspace alive.
///
/// Reads must happen outside workspace and chat updates: views read it while
/// rendering or from event handlers, never from their constructors (defer
/// the first load instead).
#[derive(Clone)]
pub(in crate::ui) struct Host {
    workspace: WeakEntity<Workspace>,
    extension: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum HostEvent {
    /// Another chat became active, or the last one closed.
    SessionOpened,
    /// A run of the active chat finished; files and quotas may differ.
    RunSettled,
    NavigationChanged,
}

impl EventEmitter<HostEvent> for Workspace {}

impl Host {
    pub(in crate::ui) fn retain_process(&self, process: AnyEntity, cx: &mut App) {
        let _ = self.workspace.update(cx, |workspace, _| {
            workspace
                .extension_processes
                .insert(self.extension, process);
        });
    }
    pub(in crate::ui) fn session_snapshot(
        &self,
        cx: &App,
    ) -> crate::declarative::protocol::Outgoing {
        let (state, entries) = self
            .read(cx, |host| {
                host.session().map(|session| {
                    (
                        Box::new(super::session::state(session, host.activity().cloned())),
                        super::session::entries(session, 0..session.transcript.entries().len()),
                    )
                })
            })
            .flatten()
            .map_or((None, Vec::new()), |(state, entries)| {
                (Some(state), entries)
            });
        crate::declarative::protocol::Outgoing::SessionSnapshot {
            session_dir: self.session_dir(cx),
            state,
            entries,
        }
    }

    pub(in crate::ui) fn subscribe_session<T: 'static>(
        &self,
        cx: &mut Context<T>,
        mut on_event: impl FnMut(&mut T, &super::session::SessionChanged, &mut Context<T>) + 'static,
    ) -> Option<Subscription> {
        Some(
            cx.subscribe(&self.workspace.upgrade()?, move |this, _, event, cx| {
                on_event(this, event, cx)
            }),
        )
    }
    pub(in crate::ui) fn identity(&self) -> EntityId {
        self.workspace.entity_id()
    }
    pub(in crate::ui) fn new(workspace: WeakEntity<Workspace>, extension: &'static str) -> Self {
        Self {
            workspace,
            extension,
        }
    }

    /// Id of the extension this host was handed to.
    pub(in crate::ui) fn extension(&self) -> &'static str {
        self.extension
    }

    /// Folder the agent works in.
    pub(in crate::ui) fn workspace_path(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx, |host| host.workspace.workspace_path.clone())
    }

    pub(in crate::ui) fn read<R>(&self, cx: &App, read: impl FnOnce(&HostView) -> R) -> Option<R> {
        let workspace = self.workspace.upgrade()?.read(cx);
        let chat = workspace.active_chat_view().map(|chat| chat.read(cx));
        Some(read(&HostView { workspace, chat }))
    }

    /// The active chat, for extensions that change its session settings.
    pub(in crate::ui) fn chat(&self, cx: &App) -> Option<Entity<Chat>> {
        let workspace = self.workspace.upgrade()?;
        workspace.read(cx).active_chat_view().cloned()
    }

    pub(in crate::ui) fn api(&self, cx: &App) -> Option<Api> {
        self.read(cx, |host| host.workspace.api()).flatten()
    }

    pub(in crate::ui) fn session_dir(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx, |host| host.session().map(|session| session.dir.clone()))
            .flatten()
    }

    pub(in crate::ui) fn active_document(&self, cx: &App) -> Option<String> {
        self.read(cx, |host| host.active_document(self.extension, cx))
            .flatten()
    }

    /// Calls `on_event` for every host event while `cx`'s entity lives.
    pub(in crate::ui) fn subscribe<T: 'static>(
        &self,
        cx: &mut Context<T>,
        mut on_event: impl FnMut(&mut T, HostEvent, &mut Context<T>) + 'static,
    ) -> Option<Subscription> {
        let workspace = self.workspace.upgrade()?;
        Some(
            cx.subscribe(&workspace, move |this, _, event: &HostEvent, cx| {
                on_event(this, *event, cx)
            }),
        )
    }

    /// Opens a document of extension `extension` as a tab and returns its
    /// view. A preview tab is replaced by the next preview until it is kept.
    pub(in crate::ui) fn open_document(
        &self,
        extension: &'static str,
        argument: &str,
        open: OpenDocument,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyView> {
        let id = crate::tabs::document(extension, argument);
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.open_document(&id, open, window, cx)
            })
            .ok()
            .flatten()
    }

    /// Brings an extension forward: opens or shows its panel.
    pub(in crate::ui) fn activate(&self, id: &'static str, window: &mut Window, cx: &mut App) {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.activate_extension(id, window, cx)
            })
            .ok();
    }
}

/// How [`Host::open_document`] shows a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) struct OpenDocument {
    /// Temporary tab, replaced by the next preview.
    pub preview: bool,
    /// Move keyboard focus to the document.
    pub focus: bool,
}

/// Read-only view of the workspace for one call of [`Host::read`].
pub(in crate::ui) struct HostView<'a> {
    workspace: &'a Workspace,
    chat: Option<&'a Chat>,
}

impl HostView<'_> {
    /// Session of the active chat.
    pub(in crate::ui) fn session(&self) -> Option<&Session> {
        self.chat.map(|chat| &chat.session)
    }

    /// Server-side activity of the active chat's session.
    pub(in crate::ui) fn activity(&self) -> Option<&AppSessionActivity> {
        let session = self.session()?;
        self.workspace
            .sessions
            .iter()
            .find(|summary| summary.session_dir == session.dir)?
            .activity
            .as_ref()
    }

    /// Argument of the document tab active in the focused pane, when the
    /// tab belongs to `extension`.
    pub(in crate::ui) fn active_document(&self, extension: &str, cx: &App) -> Option<String> {
        let layout = self.workspace.tab_layout(cx);
        let (owner, argument) = crate::tabs::split_document(layout.active(layout.focused)?)?;
        (owner == extension).then(|| argument.to_string())
    }
}
