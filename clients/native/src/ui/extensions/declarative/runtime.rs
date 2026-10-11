//! One extension process shared by all surfaces of the extension.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::*;

use super::view::DeclarativeView;
use crate::declarative::manifest::Installed;
use crate::declarative::process::{Process, Signal};
use crate::declarative::protocol::{
    HostEventKind, Incoming, Outgoing, PROTOCOL, Server, SurfaceKind,
};
use crate::ui::extensions::host::{Host, HostEvent, OpenDocument};

pub(super) struct Runtime {
    extension: &'static Installed,
    host: Host,
    window: AnyWindowHandle,
    process: Option<Process>,
    views: HashMap<String, Attached>,
    /// Why the process is not running, or its last protocol mistake.
    pub(super) error: Option<String>,
    _pump: Option<Task<()>>,
    _events: Option<Subscription>,
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
            };
            let session_dir = this.host.session_dir(cx);
            if this.process.is_none() && this.error.is_none() {
                this.start(cx);
            }
            this.send(&Outgoing::Event { event, session_dir });
        });
        Self {
            extension,
            host,
            window: window.window_handle(),
            process: None,
            views: HashMap::new(),
            error: None,
            _pump: None,
            _events: events,
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
        self.process = None;
        self._pump = None;
        self.error = None;
        self.start(cx);
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
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

    fn launch(
        &mut self,
        server: Option<Server>,
        workspace: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.process.is_some() {
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

    fn handle(&mut self, signal: Signal, cx: &mut Context<Self>) {
        match signal {
            Signal::Message(Incoming::View { view, root }) => {
                if let Some(entity) = self.views.get(&view).and_then(|a| a.view.upgrade()) {
                    entity.update(cx, |view, cx| view.show(root, cx));
                }
                self.error = None;
            }
            Signal::Message(Incoming::OpenDocument { argument }) => {
                if self.extension.manifest.surfaces.document.is_none() {
                    self.error = Some("расширение не объявило документы".into());
                } else {
                    let (host, id) = (self.host.clone(), self.extension_id());
                    let _ = self.window.update(cx, move |_, window, cx| {
                        let open = OpenDocument {
                            preview: false,
                            focus: true,
                        };
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
    super::runtime_of(host.extension(), cx)
}
