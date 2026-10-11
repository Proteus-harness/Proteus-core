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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum HostEvent {
    /// Another chat became active, or the last one closed.
    SessionOpened,
    /// A run of the active chat finished; files and quotas may differ.
    RunSettled,
}

impl EventEmitter<HostEvent> for Workspace {}

impl Host {
    pub(in crate::ui) fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace }
    }

    pub(in crate::ui) fn read<R>(&self, cx: &App, read: impl FnOnce(&HostView) -> R) -> Option<R> {
        let workspace = self.workspace.upgrade()?.read(cx);
        let chat = workspace.active_chat_view().map(|chat| chat.read(cx));
        Some(read(&HostView { workspace, chat }))
    }

    pub(in crate::ui) fn api(&self, cx: &App) -> Option<Api> {
        self.read(cx, |host| host.workspace.api()).flatten()
    }

    pub(in crate::ui) fn session_dir(&self, cx: &App) -> Option<PathBuf> {
        self.read(cx, |host| host.session().map(|session| session.dir.clone()))
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

    /// Like [`Host::subscribe`], for views that need the window.
    pub(in crate::ui) fn subscribe_in<T: 'static>(
        &self,
        window: &Window,
        cx: &mut Context<T>,
        mut on_event: impl FnMut(&mut T, HostEvent, &mut Window, &mut Context<T>) + 'static,
    ) -> Option<Subscription> {
        let workspace = self.workspace.upgrade()?;
        Some(cx.subscribe_in(
            &workspace,
            window,
            move |this, _, event: &HostEvent, window, cx| on_event(this, *event, window, cx),
        ))
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

    /// Brings an extension forward, e.g. shows its sidebar view.
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

    /// Argument of the document tab active in the focused area, when the
    /// tab belongs to `extension`.
    pub(in crate::ui) fn active_document(&self, extension: &str, cx: &App) -> Option<String> {
        let layout = self.workspace.tab_layout(cx);
        let active = layout.active(layout.focused)?;
        let (owner, argument) = crate::tabs::split_document(active)?;
        (owner == extension).then(|| argument.to_string())
    }
}
