use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::super::{
    SettingsView,
    widgets::{Tone, card, chip, hashed_id, item_row, named, note, section},
};
use crate::catalog::{
    Source, install,
    schema::{Catalog, Entry, Kind},
};
use crate::ui::{kit, prefs};

impl SettingsView {
    pub(in crate::ui::settings) fn catalog_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let busy = self.catalogs.busy;
        let mut parts = vec![section(
            "Репозитории", None, None,
            card([
                note("Git-репозитории с proteus-catalog.json в корне. Скачивание не запускает сборку или install-скрипты.", cx).into_any_element(),
                v_flex().px_4().pb_3().gap_2()
                    .child(Input::new(&self.catalogs.url).small())
                    .child(h_flex().flex_wrap().gap_2()
                        .child(Input::new(&self.catalogs.branch).small().w_48())
                        .child(kit::button("catalog-add").label("Добавить каталог").small().primary()
                            .disabled(busy).on_click(cx.listener(|this, _, _, cx| this.add_catalog(cx)))))
                    .into_any_element(),
            ], cx), cx).into_any_element()];
        for source in &prefs::current(cx).catalogs.sources {
            let found = self.catalogs.loaded.iter().find(|(old, _)| old == source);
            let title = found
                .map(|(_, catalog)| catalog.title.as_str())
                .unwrap_or(&source.url);
            let (update, forget) = (source.clone(), source.clone());
            let about = format!(
                "{}{}",
                source.url,
                source
                    .branch
                    .as_ref()
                    .map(|branch| format!(" · {branch}"))
                    .unwrap_or_default()
            );
            parts.push(
                card(
                    [item_row(
                        named(
                            hashed_id("catalog-source", source),
                            title.to_string(),
                            Some(about.into()),
                            None,
                            cx,
                        ),
                        [
                            kit::button(hashed_id("catalog-refresh", source))
                                .label("Обновить")
                                .small()
                                .ghost()
                                .loading(busy)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.refresh_catalog(update.clone(), cx)
                                }))
                                .into_any_element(),
                            kit::button(hashed_id("catalog-forget", source))
                                .label("Убрать")
                                .small()
                                .ghost()
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.forget_catalog(&forget, cx)
                                }))
                                .into_any_element(),
                        ],
                    )
                    .into_any_element()],
                    cx,
                )
                .into_any_element(),
            );
            if found.is_none() {
                parts.push(
                    note(
                        "Нет сохранённого каталога · нажмите «Обновить» для загрузки.",
                        cx,
                    )
                    .into_any_element(),
                );
            }
        }
        if let Some((error, message)) = &self.catalogs.message {
            parts.push(
                note(message.clone(), cx)
                    .when(*error, |note| note.text_color(cx.theme().danger))
                    .into_any_element(),
            );
        }
        parts.push(
            Input::new(&self.catalogs.search)
                .small()
                .cleanable(true)
                .prefix(Icon::new(IconName::Search).small())
                .into_any_element(),
        );
        let mut filters = vec![self.category_button(None, "Все категории", cx)];
        // Category ids are local to a source, not shared between repositories.
        for (source, catalog) in &self.catalogs.loaded {
            for category in &catalog.categories {
                let key = format!("{}:{}", source.key(), category.id);
                filters.push(self.category_button(Some(key), &category.title, cx));
            }
        }
        parts.push(
            h_flex()
                .flex_wrap()
                .gap_2()
                .children(filters)
                .into_any_element(),
        );
        for (source, catalog) in &self.catalogs.loaded {
            let rows = catalog
                .entries
                .iter()
                .filter(|entry| {
                    let category = format!("{}:{}", source.key(), entry.category);
                    self.catalogs
                        .category
                        .as_ref()
                        .is_none_or(|selected| *selected == category)
                        && format!(
                            "{} {} {} {}",
                            entry.id,
                            entry.title,
                            entry.description,
                            entry.kind.title()
                        )
                        .to_lowercase()
                        .contains(self.catalogs.query.trim())
                })
                .map(|entry| self.catalog_entry(source, catalog, entry, cx))
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                parts.push(
                    section(catalog.title.clone(), None, None, card(rows, cx), cx)
                        .into_any_element(),
                );
            }
            for problem in &catalog.problems {
                parts.push(
                    note(format!("{}: {problem}", catalog.title), cx)
                        .text_color(cx.theme().warning)
                        .into_any_element(),
                );
            }
        }
        parts
    }

    fn category_button(&self, key: Option<String>, title: &str, cx: &Context<Self>) -> AnyElement {
        kit::button(hashed_id("catalog-category", &key))
            .label(title.to_string())
            .small()
            .ghost()
            .selected(self.catalogs.category == key)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.catalogs.category = key.clone();
                cx.notify();
            }))
            .into_any_element()
    }

    fn catalog_entry(
        &self,
        source: &Source,
        _catalog: &Catalog,
        entry: &Entry,
        cx: &Context<Self>,
    ) -> AnyElement {
        let profile = self
            .snapshot()
            .and_then(|snapshot| snapshot.target_path.as_deref())
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new(""));
        let installed =
            install::receipt(&self.catalogs.pending, source, entry, profile).or_else(|| {
                install::receipt(
                    &prefs::current(cx).catalogs.installed,
                    source,
                    entry,
                    profile,
                )
            });
        let connected = installed.is_some_and(|old| install::connected(old, &self.draft));
        let update = installed.is_some_and(|old| old.version != entry.version);
        let needs_profile = matches!(entry.kind, Kind::AgentPlugin | Kind::McpServer);
        let unavailable = needs_profile
            && !self
                .snapshot()
                .is_some_and(|snapshot| snapshot.writable && snapshot.target_path.is_some());
        let disabled = self.catalogs.busy || self.saving || self.reloading || unavailable;
        let (add_source, add_entry) = (source.clone(), entry.clone());
        let mut controls = vec![
            chip(entry.kind.title(), Tone::Muted, cx).into_any_element(),
            chip(entry.version.clone(), Tone::Muted, cx).into_any_element(),
        ];
        if connected {
            let pending =
                installed.is_some_and(|installed| self.catalogs.pending.contains(installed));
            controls.push(
                chip(
                    if pending {
                        "В черновике"
                    } else if needs_profile {
                        "Подключено"
                    } else {
                        "Установлено"
                    },
                    Tone::Success,
                    cx,
                )
                .into_any_element(),
            );
        }
        if !connected || update {
            controls.push(
                kit::button(hashed_id("catalog-install", &(source, &entry.id)))
                    .label(if update {
                        "Обновить"
                    } else if installed.is_some() {
                        "Подключить"
                    } else {
                        "Установить"
                    })
                    .small()
                    .outline()
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.install_catalog_entry(add_source.clone(), add_entry.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
        if connected {
            let installed = installed.unwrap().clone();
            controls.push(
                kit::button(hashed_id("catalog-remove", &(source, &entry.id)))
                    .label("Удалить")
                    .small()
                    .ghost()
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_catalog_entry(installed.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
        let (open_source, open_entry) = (source.clone(), entry.clone());
        controls.push(
            kit::button(hashed_id("catalog-details", &(source, &entry.id)))
                .label("Подробнее")
                .small()
                .ghost()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_catalog_entry(open_source.clone(), open_entry.clone(), cx)
                }))
                .into_any_element(),
        );
        let selected = self
            .catalogs
            .selected
            .as_ref()
            .is_some_and(|(old, id)| old == source && *id == entry.id);
        v_flex().child(v_flex().px_4().py_2().gap_1()
            .child(named(hashed_id("catalog-name", &(source, &entry.id)), entry.title.clone(), Some(entry.description.clone().into()), Some(entry.id.clone().into()), cx))
            .child(h_flex().w_full().min_w_0().flex_wrap().gap_2().items_center().children(controls))
        ).when(selected, |row| row.child(
            v_flex().px_4().pb_4().gap_3()
                .child(note(entry.description.clone(), cx).p_0())
                .when(needs_profile, |details| details.child(note("Подключение меняет черновик текущего профиля. Нажмите «Сохранить». Файлы Agent Plugins остаются на диске после отключения.", cx).p_0()))
                .when(unavailable, |details| details.child(note("Нужен загруженный доступный для записи профиль.", cx).p_0()))
                .children(self.catalogs.readme.as_ref().map(|state| TextView::new(state)))
        )).into_any_element()
    }
}
