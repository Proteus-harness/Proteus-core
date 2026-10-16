//! Installing and removing catalog entries. Client extensions and skills
//! land in client folders; Agent Plugins and MCP servers are written into
//! the profile at once, past the settings draft.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use gpui_kit::*;

use super::super::{SettingsEvent, SettingsView};
use super::in_profile;
use crate::catalog::{
    Installed, Locations, Source, git, install,
    schema::{Entry, Kind},
};
use crate::settings::Draft;
use crate::ui::{extensions, prefs};

impl SettingsView {
    /// Why entries of this kind cannot change now.
    pub(super) fn catalog_blocked(&self, kind: Kind) -> Option<&'static str> {
        in_profile(kind).then(|| self.profile_blocked()).flatten()
    }

    /// Installing also reads the source's checkout, which a download replaces.
    pub(super) fn catalog_install_blocked(
        &self,
        source: &Source,
        kind: Kind,
    ) -> Option<&'static str> {
        self.catalog_install_gate(source, kind, self.profile_blocked())
    }

    /// The same check with `profile`, the verdict of `profile_blocked`,
    /// worked out once for a whole list: it compares the entire draft.
    pub(super) fn catalog_install_gate(
        &self,
        source: &Source,
        kind: Kind,
        profile: Option<&'static str>,
    ) -> Option<&'static str> {
        if self.catalogs.loading.contains(source) {
            return Some("Источник сейчас скачивается");
        }
        in_profile(kind).then_some(profile).flatten()
    }

    pub(super) fn profile_blocked(&self) -> Option<&'static str> {
        if !self
            .snapshot()
            .is_some_and(|snapshot| snapshot.writable && snapshot.target_path.is_some())
        {
            return Some("Нужен загруженный профиль, доступный для записи");
        }
        self.dirty()
            .then_some("Сначала сохраните или сбросьте изменения на других страницах настроек")
    }

    pub(super) fn catalog_idle(&self) -> bool {
        !(self.catalogs.busy || self.saving || self.reloading)
    }

    pub(super) fn install_catalog_entry(
        &mut self,
        source: Source,
        entry: Entry,
        cx: &mut Context<Self>,
    ) {
        if !self.catalog_idle() {
            return;
        }
        if let Some(reason) = self.catalog_install_blocked(&source, entry.kind) {
            self.catalog_message(Err(anyhow!(reason)));
            cx.notify();
            return;
        }
        let profile = self
            .snapshot()
            .and_then(|snapshot| snapshot.target_path.as_deref())
            .map(PathBuf::from)
            .unwrap_or_default();
        let previous = install::receipt(
            &prefs::current(cx).catalogs.installed,
            &source,
            &entry,
            &profile,
        )
        .cloned();
        let Some((_, catalog)) = self.catalogs.loaded.iter().find(|(old, _)| *old == source) else {
            return;
        };
        let catalog = catalog.clone();
        let taken = extensions::ids()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        self.catalogs.busy = true;
        self.catalogs.working = Some((source.clone(), entry.id.clone()));
        self.catalogs.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let old = previous.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let locations = Locations::local()?;
                    let folder = [entry.path.clone()];
                    git::with_files(&locations.cache, &source, &folder, || {
                        install::prepare(
                            &locations,
                            &source,
                            &catalog,
                            &entry,
                            old.as_ref(),
                            &profile,
                            &taken,
                        )
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let installed = match result {
                    Ok(installed) => installed,
                    Err(error) => return this.catalog_failed(error, cx),
                };
                if in_profile(installed.kind) {
                    let receipt = installed.clone();
                    return this.write_catalog_profile(
                        move |draft| install::connect(draft, &installed, previous.as_ref()),
                        move |cx| prefs::update(cx, |prefs| prefs.catalogs.remember(receipt)),
                        "Установлено · профиль сохранён",
                        cx,
                    );
                }
                this.catalogs.busy = false;
                let kind = installed.kind;
                prefs::update(cx, |prefs| prefs.catalogs.remember(installed));
                this.reload_installed(kind, cx);
                this.catalog_message(Ok("Установлено"));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remove_catalog_entry(&mut self, installed: Installed, cx: &mut Context<Self>) {
        if !self.catalog_idle() {
            return;
        }
        if let Some(reason) = self.catalog_blocked(installed.kind) {
            self.catalog_message(Err(anyhow!(reason)));
            cx.notify();
            return;
        }
        self.catalogs.busy = true;
        self.catalogs.working = Some((installed.source.clone(), installed.entry.clone()));
        self.catalogs.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let files = installed.clone();
            let result = cx
                .background_executor()
                .spawn(async move { install::remove_files(&Locations::local()?, &files) })
                .await;
            this.update(cx, |this, cx| {
                if let Err(error) = result {
                    return this.catalog_failed(error, cx);
                }
                if in_profile(installed.kind) {
                    let receipt = installed.clone();
                    return this.write_catalog_profile(
                        move |draft| install::disconnect(draft, &installed),
                        move |cx| {
                            prefs::update(cx, |prefs| {
                                prefs.catalogs.installed.retain(|old| *old != receipt)
                            })
                        },
                        "Удалено из профиля",
                        cx,
                    );
                }
                this.catalogs.busy = false;
                prefs::update(cx, |prefs| {
                    prefs.catalogs.installed.retain(|old| *old != installed);
                    if installed.kind == Kind::ClientExtension {
                        prefs.extensions.forget(&installed.entry);
                    }
                });
                this.reload_installed(installed.kind, cx);
                this.catalog_message(Ok("Удалено"));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Saves `change` applied to the saved profile, not to the draft, and
    /// keeps the receipt only when the profile accepted it.
    fn write_catalog_profile(
        &mut self,
        change: impl FnOnce(&mut Draft) -> Result<()>,
        saved: impl FnOnce(&mut App) + 'static,
        done: &'static str,
        cx: &mut Context<Self>,
    ) {
        let mut draft = self.baseline.clone();
        let check = match self.profile_blocked() {
            Some(reason) => Err(anyhow!(reason)),
            None => change(&mut draft),
        };
        if let Err(error) = check {
            return self.catalog_failed(error, cx);
        }
        if draft == self.baseline {
            self.catalogs.busy = false;
            saved(cx);
            self.catalog_message(Ok(done));
            cx.notify();
            return;
        }
        let request = draft.changes(&self.baseline);
        let api = self.api.clone();
        let dir = self.session_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { api.save_config_builder(&dir, &request) })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                match result {
                    Ok(snapshot) => {
                        saved(cx);
                        this.accept(snapshot);
                        this.load_addons(cx);
                        this.refresh_history(cx);
                        this.catalog_message(Ok(done));
                        cx.emit(SettingsEvent::Saved);
                    }
                    Err(error) => this.catalog_message(Err(error.context("Профиль не сохранён"))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload_installed(&mut self, kind: Kind, cx: &mut Context<Self>) {
        match kind {
            Kind::ClientExtension => extensions::reload_installed(cx),
            Kind::Skill => self.load_addons(cx),
            Kind::AgentPlugin | Kind::McpServer => {}
        }
    }

    fn catalog_failed(&mut self, error: anyhow::Error, cx: &mut Context<Self>) {
        self.catalogs.busy = false;
        self.catalog_message(Err(error));
        cx.notify();
    }
}
