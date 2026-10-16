//! One extension process shared by all surfaces of the extension.

mod commands;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use gpui_kit::*;

use super::view::DeclarativeView;
use crate::declarative::manifest::Installed;
use crate::declarative::presentation::Presentation;
use crate::declarative::process::{Process, Signal};
use crate::declarative::protocol::{
    HostEventKind, Incoming, Outgoing, PROTOCOL, Server, SurfaceKind,
};
use crate::ui::extensions::host::{Host, HostEvent, OpenDocument};

pub(super) struct Runtime {
    generation: u64,
    settings: VecDeque<commands::Setting>,
    setting_running: bool,
    extension: &'static Installed,
    host: Host,
    window: AnyWindowHandle,
    process: Option<Process>,
    views: HashMap<String, Attached>,
    /// Why the process is not running, or its last protocol mistake.
    pub(super) error: Option<String>,
    pub(super) popup_closed: u64,
    _pump: Option<Task<()>>,
    _events: Option<Subscription>,
    _session_events: Option<Subscription>,
    _preferences: Subscription,
}

struct Attached {
    view: WeakEntity<DeclarativeView>,
    surface: SurfaceKind,
    argument: Option<String>,
}

impl Runtime {
    pub(super) fn new(
        extension: &'static Installed,
        host: Host,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let events = host.subscribe(cx, |this: &mut Self, event, cx| {
            let event = match event {
                HostEvent::SessionOpened => HostEventKind::SessionOpened,
                HostEvent::RunSettled => HostEventKind::RunSettled,
                HostEvent::NavigationChanged => HostEventKind::NavigationChanged,
            };
            let session_dir = this.host.session_dir(cx);
            if event == HostEventKind::SessionOpened {
                for attached in this
                    .views
                    .values()
                    .filter(|attached| attached.surface == SurfaceKind::Badge)
                {
                    if let Some(view) = attached.view.upgrade() {
                        view.update(cx, |view, cx| view.present(None, cx));
                    }
                }
            }
            if this.process.is_none() && this.error.is_none() {
                this.start(cx);
            }
            this.send(&Outgoing::Event {
                event,
                session_dir,
                document: this.host.active_document(cx),
            });
            if event == HostEventKind::SessionOpened && this.extension.manifest.session_events {
                this.send(&this.host.session_snapshot(cx));
            }
        });
        let session_events = host.subscribe_session(cx, |this: &mut Self, event, _| {
            if this.extension.manifest.session_events {
                this.send(&Outgoing::SessionUpdate {
                    session_dir: event.session_dir.clone(),
                    update: event.update.clone(),
                });
            }
        });
        Self {
            generation: 0,
            settings: VecDeque::new(),
            setting_running: false,
            extension,
            host,
            window: window.window_handle(),
            process: None,
            views: HashMap::new(),
            error: None,
            popup_closed: 0,
            _pump: None,
            _events: events,
            _session_events: session_events,
            _preferences: cx.observe_global::<crate::ui::prefs::ClientPreferences>(|this, cx| {
                if super::super::is_enabled(this.extension_id(), cx) {
                    if this.process.is_none() && this.error.is_none() {
                        this.start(cx);
                    }
                } else {
                    this.stop(cx);
                }
            }),
        }
    }

    pub(super) fn title(&self) -> &'static str {
        &self.extension.manifest.title
    }

    pub(super) fn attach(
        &mut self,
        view: String,
        surface: SurfaceKind,
        argument: Option<String>,
        entity: WeakEntity<DeclarativeView>,
        cx: &mut Context<Self>,
    ) {
        self.views.insert(
            view.clone(),
            Attached {
                view: entity,
                surface,
                argument,
            },
        );
        if self.process.is_some() {
            self.render(&view, cx);
        } else if self.error.is_none() {
            self.start(cx);
        }
    }

    pub(super) fn detach(&mut self, view: &str) {
        if self.views.remove(view).is_some() {
            self.send(&Outgoing::Close { view: view.into() });
        }
    }

    pub(super) fn send(&self, message: &Outgoing) {
        if let Some(process) = &self.process {
            process.send(message);
        }
    }

    /// Starts the process again, e.g. after the user fixed it.
    pub(super) fn restart(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        self.error = None;
        self.start(cx);
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !super::super::is_enabled(self.extension_id(), cx) {
            return;
        }
        // Read the host outside this update: it lives in the workspace.
        let host = self.host.clone();
        cx.defer(move |cx| {
            let server = host.api(cx).map(|api| Server {
                origin: api.origin().to_string(),
                token: api.token().to_string(),
            });
            let workspace = host.workspace_path(cx);
            let Some(this) = host_runtime(cx, &host) else {
                return;
            };
            this.update(cx, |this, cx| this.launch(server, workspace, cx));
        });
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let current = super::installed(self.extension_id());
        if current.is_some_and(|extension| std::ptr::eq(extension, self.extension)) {
            return;
        }
        self.stop(cx);
        match current {
            Some(extension) => {
                self.extension = extension;
                self.error = None;
                self.start(cx);
            }
            None => self.error = Some("расширение удалено".into()),
        }
        cx.notify();
    }

    fn launch(
        &mut self,
        server: Option<Server>,
        workspace: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.process.is_some() || !super::super::is_enabled(self.extension_id(), cx) {
            return;
        }
        let (Some(server), Some(workspace)) = (server, workspace) else {
            // Not connected yet; the next host event starts it.
            return;
        };
        match Process::spawn(self.extension) {
            Ok((process, signals)) => {
                process.send(&Outgoing::Hello {
                    protocol: PROTOCOL,
                    workspace,
                    server,
                });
                self.process = Some(process);
                self.send(&Outgoing::Event {
                    event: HostEventKind::NavigationChanged,
                    session_dir: self.host.session_dir(cx),
                    document: self.host.active_document(cx),
                });
                if self.extension.manifest.session_events {
                    self.send(&self.host.session_snapshot(cx));
                }
                self._pump = Some(cx.spawn(async move |this, cx| {
                    while let Ok(signal) = signals.recv().await {
                        if this.update(cx, |this, cx| this.handle(signal, cx)).is_err() {
                            break;
                        }
                    }
                }));
                let views: Vec<String> = self.views.keys().cloned().collect();
                for view in views {
                    self.render(&view, cx);
                }
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn render(&self, view: &str, cx: &App) {
        let Some(attached) = self.views.get(view) else {
            return;
        };
        self.send(&Outgoing::Render {
            view: view.into(),
            surface: attached.surface,
            argument: attached.argument.clone(),
            session_dir: self.host.session_dir(cx),
        });
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.settings.clear();
        self.popup_closed += 1;
        self.process = None;
        self._pump = None;
        for attached in self.views.values() {
            if let Some(view) = attached.view.upgrade() {
                view.update(cx, |view, cx| view.reset(cx));
            }
        }
    }

    fn handle(&mut self, signal: Signal, cx: &mut Context<Self>) {
        match signal {
            Signal::Message(Incoming::Focus { view, control }) => {
                if let Some(view) = self
                    .views
                    .get(&view)
                    .and_then(|attached| attached.view.upgrade())
                {
                    let host = self.host.clone();
                    let _ = self.window.update(cx, move |_, window, cx| {
                        if let Some(control) = control {
                            view.update(cx, |view, cx| view.focus_control(&control, window, cx));
                        } else if let Some(chat) = host.chat(cx) {
                            chat.update(cx, |chat, cx| chat.focus(window, cx));
                        }
                    });
                }
            }
            Signal::Message(Incoming::ClosePopup) => self.popup_closed += 1,
            Signal::Message(Incoming::Clipboard { text }) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            Signal::Message(Incoming::SetSession {
                id,
                session_dir,
                setting,
            }) => self.set_session(id, session_dir, setting, cx),
            Signal::Message(Incoming::Badge {
                hidden,
                view,
                session_dir,
                title,
                lines,
                face,
            }) => {
                if session_dir != self.host.session_dir(cx) {
                    return;
                }
                let Some(attached) = self
                    .views
                    .get(&view)
                    .filter(|view| view.surface == SurfaceKind::Badge)
                else {
                    self.error = Some("badge message requires an attached badge surface".into());
                    cx.notify();
                    return;
                };
                let narrow = self
                    .extension
                    .manifest
                    .surfaces
                    .badge
                    .as_ref()
                    .is_some_and(|badge| badge.narrow);
                if narrow && !face.narrow() {
                    self.error = Some("a narrow badge cannot display text".into());
                } else if let Some(entity) = attached.view.upgrade() {
                    entity.update(cx, |view, cx| {
                        view.present(
                            Some(Presentation {
                                hidden,
                                title,
                                lines,
                                face,
                            }),
                            cx,
                        )
                    });
                    self.error = None;
                }
            }
            Signal::Message(Incoming::View {
                keys,
                view,
                root,
                sequence,
                session_dir,
            }) => {
                if session_dir != self.host.session_dir(cx) {
                    return;
                }
                let narrow = self
                    .extension
                    .manifest
                    .surfaces
                    .badge
                    .as_ref()
                    .is_some_and(|badge| badge.narrow);
                if narrow
                    && self
                        .views
                        .get(&view)
                        .is_some_and(|attached| attached.surface == SurfaceKind::Badge)
                {
                    self.error = Some("narrow badges require a badge presentation".into());
                    cx.notify();
                    return;
                }
                if let Some(entity) = self.views.get(&view).and_then(|a| a.view.upgrade()) {
                    entity.update(cx, |view, cx| view.show(root, sequence, keys, cx));
                }
                self.error = None;
            }
            Signal::Message(Incoming::OpenDocument {
                argument,
                preview,
                focus,
            }) => {
                if self.extension.manifest.surfaces.document.is_none() {
                    self.error = Some("расширение не объявило документы".into());
                } else {
                    let (host, id) = (self.host.clone(), self.extension_id());
                    let _ = self.window.update(cx, move |_, window, cx| {
                        let open = OpenDocument { preview, focus };
                        host.open_document(id, &argument, open, window, cx);
                    });
                }
            }
            Signal::Invalid(error) => self.error = Some(format!("неверное сообщение: {error}")),
            Signal::Exited(text) => {
                self.process = None;
                self.error = Some(text);
            }
        }
        cx.notify();
    }

    fn extension_id(&self) -> &'static str {
        self.extension.manifest.id.as_str()
    }
}

fn host_runtime(cx: &App, host: &Host) -> Option<Entity<Runtime>> {
    super::runtime_of(host, cx)
}
