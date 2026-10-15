//! Agent Plugins archive import uses the same file handling as extension ZIPs.

use gpui_kit::*;

use super::super::SettingsView;

impl SettingsView {
    pub(in crate::ui::settings) fn choose_package_zip(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Установить ZIP".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let started = this
                .update(cx, |this, cx| {
                    if this.catalogs.busy || this.saving {
                        return false;
                    }
                    this.catalogs.busy = true;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !started {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::catalog::install::import_zip(&crate::catalog::Locations::local()?, &path)
                })
                .await;
            this.update(cx, |this, cx| {
                this.catalogs.busy = false;
                match result {
                    Ok(path) => {
                        this.update_draft(cx, |draft| draft.add_package(path));
                        this.show_feedback("Пакет распакован · нажмите «Сохранить»", false, cx);
                    }
                    Err(error) => {
                        this.show_feedback(format!("ZIP не установлен: {error:#}"), true, cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
