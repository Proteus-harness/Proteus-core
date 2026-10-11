//! Analytics pages shown in the settings screen: «Расход и контекст»,
//! «Анализ ходов» and «Архитектура». They read the agent only through the
//! public HTTP API and the headless `crate::analytics` modules.

mod analysis;
mod architecture;
mod picker;
mod usage;
mod widgets;

use std::path::PathBuf;

use gpui_kit::*;

use crate::api::Api;
use crate::settings::catalog::Overview;

/// The three pages; built once per settings screen, fetched on first show.
pub struct Pages {
    usage: Entity<usage::UsagePage>,
    analysis: Entity<analysis::AnalysisPage>,
    architecture: Entity<architecture::ArchitecturePage>,
}

impl Pages {
    pub fn new(api: Api, session_dir: PathBuf, window: &mut Window, cx: &mut App) -> Self {
        let picker = cx.new(|cx| picker::SessionPicker::new(api.clone(), session_dir.clone(), cx));
        Self {
            usage: cx.new(|cx| usage::UsagePage::new(api.clone(), picker.clone(), window, cx)),
            analysis: cx.new(|cx| analysis::AnalysisPage::new(api.clone(), picker, window, cx)),
            architecture: cx
                .new(|cx| architecture::ArchitecturePage::new(api, session_dir, window, cx)),
        }
    }

    pub fn view(&self, page: Overview) -> AnyView {
        match page {
            Overview::Usage => self.usage.clone().into(),
            Overview::Analysis => self.analysis.clone().into(),
            Overview::Architecture => self.architecture.clone().into(),
        }
    }
}
