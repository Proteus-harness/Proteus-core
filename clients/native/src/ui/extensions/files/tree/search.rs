use std::time::Duration;

use gpui_kit::*;

use super::FilesPanel;

/// Pause after the last keystroke before asking the server.
const DEBOUNCE: Duration = Duration::from_millis(150);

impl FilesPanel {
    /// The tree currently shows server matches rather than listings.
    pub(super) fn searching(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|result| result.query == self.query)
    }

    /// Asks the server for matches of the current query once typing pauses.
    pub(super) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.query.clone();
        if query.is_empty() {
            self.search = None;
            self.search_error = None;
            return;
        }
        let Some((api, dir)) = self.connection(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let current = this
                .read_with(cx, |this, _| this.search_generation == generation)
                .unwrap_or(false);
            if !current {
                return;
            }
            let job_dir = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move { api.workspace_search(&job_dir, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation || this.session_dir.as_ref() != Some(&dir) {
                    return;
                }
                match result {
                    Ok(result) => {
                        this.search = Some(result);
                        this.search_error = None;
                    }
                    Err(error) => {
                        this.search_error = Some(format!("Поиск не удался: {error:#}").into())
                    }
                }
                this.sync_rows(cx);
            });
        })
        .detach();
    }

    /// Leaves the search and opens the directory in the regular tree.
    pub(super) fn show_in_tree(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.query.clear();
        self.schedule_search(cx);
        self.tree.reveal(&format!("{path}/"));
        self.cursor = Some(path.to_string());
        self.sync_rows(cx);
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.list.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }
}
