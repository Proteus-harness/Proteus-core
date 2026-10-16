//! Entries of every loaded catalog with their install state, filtered by the
//! search and the toolbar.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;

use super::{SettingsView, Show};
use crate::catalog::{
    Installed, Source, install,
    schema::{Catalog, Entry, Kind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Available,
    /// Files and receipt exist, the profile no longer references them.
    Detached,
    Update,
    Installed,
}

pub(super) struct Item<'a> {
    pub source: &'a Source,
    pub catalog: &'a Catalog,
    pub entry: &'a Entry,
    pub installed: Option<&'a Installed>,
    pub status: Status,
}

impl Item<'_> {
    pub fn key(&self) -> (&Source, &str) {
        (self.source, &self.entry.id)
    }

    pub fn category(&self) -> Option<&str> {
        self.catalog
            .categories
            .iter()
            .find(|category| category.id == self.entry.category)
            .map(|category| category.title.as_str())
    }

    fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || [
                self.entry.id.as_str(),
                &self.entry.title,
                &self.entry.description,
                self.entry.kind.title(),
                self.category().unwrap_or_default(),
                &self.catalog.title,
            ]
            .iter()
            .any(|text| text.to_lowercase().contains(query))
    }
}

pub(super) const KINDS: [Kind; 4] = [
    Kind::ClientExtension,
    Kind::AgentPlugin,
    Kind::Skill,
    Kind::McpServer,
];

pub(super) fn kind_icon(kind: Kind) -> Icon {
    Icon::new(match kind {
        Kind::ClientExtension => Lucide::Blocks,
        Kind::AgentPlugin => Lucide::Puzzle,
        Kind::Skill => Lucide::Sparkles,
        Kind::McpServer => Lucide::Server,
    })
}

/// Short plural for the type filter.
pub(super) fn kind_filter(kind: Kind) -> &'static str {
    match kind {
        Kind::ClientExtension => "Расширения",
        Kind::AgentPlugin => "Agent Plugins",
        Kind::Skill => "Skills",
        Kind::McpServer => "MCP",
    }
}

impl SettingsView {
    /// Every entry of the loaded catalogs; `installed` are the receipts.
    pub(super) fn catalog_items<'a>(&'a self, installed: &'a [Installed]) -> Vec<Item<'a>> {
        let profile = self
            .snapshot()
            .and_then(|snapshot| snapshot.target_path.as_deref())
            .map(std::path::Path::new)
            .unwrap_or(std::path::Path::new(""));
        self.catalogs
            .loaded
            .iter()
            .flat_map(|(source, catalog)| {
                catalog.entries.iter().map(move |entry| {
                    let installed = install::receipt(installed, source, entry, profile);
                    let status = match installed {
                        None => Status::Available,
                        Some(old) if !install::connected(old, &self.baseline) => Status::Detached,
                        Some(old) if old.version != entry.version => Status::Update,
                        Some(_) => Status::Installed,
                    };
                    Item {
                        source,
                        catalog,
                        entry,
                        installed,
                        status,
                    }
                })
            })
            .collect()
    }

    /// Whether the toolbar and the search leave the entry visible.
    pub(super) fn catalog_item_shown(&self, item: &Item) -> bool {
        (match self.catalogs.show {
            Show::Overview | Show::All => true,
            Show::Installed => matches!(item.status, Status::Installed | Status::Update),
            Show::Updates => item.status == Status::Update,
        }) && self
            .catalogs
            .kind
            .is_none_or(|kind| item.entry.kind == kind)
            && self.catalogs.scope.as_ref().is_none_or(|scope| {
                *item.source == scope.source
                    && scope
                        .category
                        .as_ref()
                        .is_none_or(|category| item.entry.category == *category)
            })
            && item.matches(self.catalogs.query.trim())
    }

    /// The overview stands in for the list until something narrows it.
    pub(super) fn catalog_browsing(&self) -> bool {
        self.catalogs.show != Show::Overview
            || self.catalogs.kind.is_some()
            || self.catalogs.scope.is_some()
            || !self.catalogs.query.trim().is_empty()
    }

    /// Rows the arrows walk: the list, or the rows of the overview.
    pub(super) fn catalog_listed<'a, 'b>(&self, items: &'b [Item<'a>]) -> Vec<&'b Item<'a>> {
        if self.catalog_browsing() {
            return items
                .iter()
                .filter(|item| self.catalog_item_shown(item))
                .collect();
        }
        let (updates, installed) = overview_rows(items);
        updates.into_iter().chain(installed).collect()
    }
}

/// Rows the overview lists in full before linking to the rest.
pub(super) const OVERVIEW_ROWS: usize = 5;

/// The first updates and installed entries the overview shows.
pub(super) fn overview_rows<'a, 'b>(
    items: &'b [Item<'a>],
) -> (Vec<&'b Item<'a>>, Vec<&'b Item<'a>>) {
    let pick = |status| {
        items
            .iter()
            .filter(|item| item.status == status)
            .take(OVERVIEW_ROWS)
            .collect()
    };
    (pick(Status::Update), pick(Status::Installed))
}
