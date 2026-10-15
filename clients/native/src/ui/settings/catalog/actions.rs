use gpui_kit::*;

use super::super::SettingsView;
use crate::catalog::{
    Locations, Source, git, install,
    schema::{self, Entry, Kind},
};
use crate::ui::{extensions, prefs};

impl SettingsView {
    pub(in crate::ui::settings) fn open_catalogs(&mut self, cx: &mut Context<Self>) {
        if self.catalogs.opened {
            return;
        }
        self.catalogs.opened = true;
        let sources = prefs::current(cx).catalogs.sources.clone();
        self.catalogs.busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let locations = Locations::local()?;
                    Ok::<_, anyhow::Error>(
                        sources
                            .into_iter()
                            .map(|source| {
                                let catalog = git::cached(&locations.cache, &source);
                                (source, catalog)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                match result {
                    Ok(catalogs) => {
                        for (source, catalog) in catalogs {
                            if let Ok(catalog) = catalog {
                                this.catalogs.loaded.push((source, catalog));
                            }
                        }
                    }
                    Err(error) => this.catalog_message(Err(error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(in crate::ui::settings) fn add_catalog(&mut self, cx: &mut Context<Self>) {
        let url = self.catalogs.url.read(cx).value().trim().to_string();
        let branch = self.catalogs.branch.read(cx).value().trim().to_string();
        let source = Source {
            url,
            branch: (!branch.is_empty()).then_some(branch),
        };
        if let Err(error) = git::validate(&source) {
            self.catalog_message(Err(error));
            cx.notify();
            return;
        }
        prefs::update(cx, |prefs| {
            if !prefs.catalogs.sources.contains(&source) {
                prefs.catalogs.sources.push(source.clone());
            }
        });
        self.refresh_catalog(source, cx);
    }

    pub(in crate::ui::settings) fn refresh_catalog(
        &mut self,
        source: Source,
        cx: &mut Context<Self>,
    ) {
        if self.catalogs.busy {
            return;
        }
        self.catalogs.busy = true;
        self.catalogs.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let copied = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move { git::refresh(&Locations::local()?.cache, &copied) })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                match result {
                    Ok(catalog) => {
                        this.catalogs.loaded.retain(|(old, _)| *old != source);
                        this.catalogs.loaded.push((source.clone(), catalog));
                        this.catalogs.selected = None;
                        this.catalogs.readme = None;
                        this.catalogs.category = None;
                        this.catalog_message(Ok("Каталог обновлён"));
                    }
                    Err(error) => this.catalog_message(Err(error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(in crate::ui::settings) fn forget_catalog(
        &mut self,
        source: &Source,
        cx: &mut Context<Self>,
    ) {
        prefs::update(cx, |prefs| {
            prefs.catalogs.sources.retain(|old| old != source)
        });
        self.catalogs.loaded.retain(|(old, _)| old != source);
        self.catalogs.selected = None;
        self.catalogs.readme = None;
        self.catalogs.category = None;
        cx.notify();
    }

    pub(in crate::ui::settings) fn select_catalog_entry(
        &mut self,
        source: Source,
        entry: Entry,
        cx: &mut Context<Self>,
    ) {
        let Some((_, catalog)) = self.catalogs.loaded.iter().find(|(old, _)| *old == source) else {
            return;
        };
        let catalog = catalog.clone();
        self.catalogs.selected = Some((source.clone(), entry.id.clone()));
        self.catalogs.readme = None;
        self.catalogs.readme_changes = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id = entry.id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { schema::readme(&catalog, &entry) })
                .await;
            this.update(cx, |this, cx| {
                if this.catalogs.selected.as_ref() != Some(&(source, id)) {
                    return;
                }
                match result {
                    Ok(Some(text)) => {
                        let state = cx.new(|cx| {
                            gpui_kit::component::text::TextViewState::markdown(&text, cx)
                        });
                        this.catalogs.readme_changes =
                            Some(cx.observe(&state, |_, _, cx| cx.notify()));
                        this.catalogs.readme = Some(state);
                    }
                    Ok(None) => {}
                    Err(error) => this.catalog_message(Err(error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(in crate::ui::settings) fn install_catalog_entry(
        &mut self,
        source: Source,
        entry: Entry,
        cx: &mut Context<Self>,
    ) {
        if self.catalogs.busy || self.saving || self.reloading {
            return;
        }
        let profile = self
            .snapshot()
            .and_then(|snapshot| snapshot.target_path.as_deref())
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        if matches!(entry.kind, Kind::McpServer | Kind::AgentPlugin)
            && (profile.as_os_str().is_empty()
                || !self.snapshot().is_some_and(|snapshot| snapshot.writable))
        {
            return;
        }
        let previous = install::receipt(&self.catalogs.pending, &source, &entry, &profile)
            .or_else(|| {
                install::receipt(
                    &prefs::current(cx).catalogs.installed,
                    &source,
                    &entry,
                    &profile,
                )
            })
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
        self.catalogs.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let old = previous.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    install::prepare(
                        &Locations::local()?,
                        &source,
                        &catalog,
                        &entry,
                        old.as_ref(),
                        &profile,
                        &taken,
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                let result = result.and_then(|installed| {
                    let mut draft = this.draft.clone();
                    install::connect(&mut draft, &installed, previous.as_ref())?;
                    if draft != this.draft {
                        this.replace_draft(draft, cx);
                    }
                    if installed.kind == Kind::ClientExtension {
                        extensions::reload_installed(cx);
                    }
                    if installed.kind == Kind::Skill {
                        this.load_addons(cx);
                    }
                    let immediate = matches!(installed.kind, Kind::ClientExtension | Kind::Skill);
                    if immediate {
                        prefs::update(cx, |prefs| prefs.catalogs.remember(installed));
                    } else {
                        this.catalogs.pending.retain(|old| {
                            old.source != installed.source
                                || old.entry != installed.entry
                                || old.profile != installed.profile
                        });
                        this.catalogs.pending.push(installed);
                    }
                    Ok(if immediate {
                        "Установлено"
                    } else {
                        "Добавлено в черновик · нажмите «Сохранить»"
                    })
                });
                this.catalog_message(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(in crate::ui::settings) fn remove_catalog_entry(
        &mut self,
        installed: crate::catalog::Installed,
        cx: &mut Context<Self>,
    ) {
        if self.catalogs.busy || self.saving || self.reloading {
            return;
        }
        self.catalogs.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let files = installed.clone();
            let result = cx
                .background_executor()
                .spawn(async move { install::remove_files(&Locations::local()?, &files) })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                let result = result.and_then(|_| {
                    let mut draft = this.draft.clone();
                    install::disconnect(&mut draft, &installed)?;
                    if draft != this.draft {
                        this.replace_draft(draft, cx);
                    }
                    let immediate = matches!(installed.kind, Kind::ClientExtension | Kind::Skill);
                    if immediate {
                        prefs::update(cx, |prefs| {
                            prefs.catalogs.installed.retain(|old| *old != installed)
                        });
                    }
                    if installed.kind == Kind::ClientExtension {
                        prefs::update(cx, |prefs| prefs.extensions.forget(&installed.entry));
                        extensions::reload_installed(cx);
                    }
                    if installed.kind == Kind::Skill {
                        this.load_addons(cx);
                    }
                    Ok(if immediate {
                        "Удалено"
                    } else {
                        "Убрано из черновика · файлы пакета сохранены, нажмите «Сохранить»"
                    })
                });
                this.catalog_message(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn catalog_message(&mut self, result: anyhow::Result<&str>) {
        self.catalogs.message = Some(match result {
            Ok(text) => (false, text.to_string()),
            Err(error) => (true, format!("{error:#}")),
        });
    }
}
