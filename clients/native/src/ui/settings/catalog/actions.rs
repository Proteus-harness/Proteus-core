use gpui_kit::*;

use super::super::SettingsView;
use crate::catalog::{
    Locations, Source, git,
    schema::{self, Entry},
};
use crate::ui::prefs;

impl SettingsView {
    pub(in crate::ui::settings) fn open_catalogs(&mut self, cx: &mut Context<Self>) {
        if self.catalogs.opened {
            return;
        }
        self.catalogs.opened = true;
        let sources = prefs::current(cx).catalogs.sources.clone();
        self.catalogs.busy = true;
        self.catalogs.working = None;
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

    pub(super) fn add_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        for input in [&self.catalogs.url, &self.catalogs.branch] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.add_catalog_sources(vec![source], cx);
    }

    /// Remembers the sources and downloads them.
    pub(super) fn add_catalog_sources(&mut self, sources: Vec<Source>, cx: &mut Context<Self>) {
        prefs::update(cx, |prefs| {
            for source in &sources {
                if !prefs.catalogs.sources.contains(source) {
                    prefs.catalogs.sources.push(source.clone());
                }
            }
        });
        self.refresh_catalogs(sources, cx);
    }

    /// Downloads the sources again, one after another.
    pub(super) fn refresh_catalogs(&mut self, sources: Vec<Source>, cx: &mut Context<Self>) {
        if self.catalogs.busy || sources.is_empty() {
            return;
        }
        self.catalogs.busy = true;
        self.catalogs.working = None;
        self.catalogs.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    let cache = Locations::local().map(|locations| locations.cache);
                    sources
                        .into_iter()
                        .map(|source| {
                            let catalog = match &cache {
                                Ok(cache) => git::refresh(cache, &source),
                                Err(error) => Err(anyhow::anyhow!("{error:#}")),
                            };
                            (source, catalog)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                let mut failed = Vec::new();
                for (source, result) in results {
                    match result {
                        Ok(catalog) => {
                            match this
                                .catalogs
                                .loaded
                                .iter_mut()
                                .find(|(old, _)| *old == source)
                            {
                                Some((_, old)) => *old = catalog,
                                None => this.catalogs.loaded.push((source, catalog)),
                            }
                        }
                        Err(error) => failed.push(format!("{}: {error:#}", source.url)),
                    }
                }
                this.catalog_message(if failed.is_empty() {
                    Ok("Каталог обновлён")
                } else {
                    Err(anyhow::anyhow!(failed.join("\n")))
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn forget_catalog(&mut self, source: &Source, cx: &mut Context<Self>) {
        prefs::update(cx, |prefs| {
            prefs.catalogs.sources.retain(|old| old != source)
        });
        self.catalogs.loaded.retain(|(old, _)| old != source);
        if self
            .catalogs
            .selected
            .as_ref()
            .is_some_and(|(old, _)| old == source)
        {
            self.deselect_catalog_entry(cx);
        }
        cx.notify();
    }

    pub(super) fn select_catalog_entry(
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

    pub(in crate::ui::settings) fn deselect_catalog_entry(&mut self, cx: &mut Context<Self>) {
        self.catalogs.selected = None;
        self.catalogs.readme = None;
        self.catalogs.readme_changes = None;
        cx.notify();
    }

    pub(in crate::ui::settings) fn catalog_message(&mut self, result: anyhow::Result<&str>) {
        self.catalogs.message = Some(match result {
            Ok(text) => (false, text.to_string()),
            Err(error) => (true, format!("{error:#}")),
        });
    }
}
