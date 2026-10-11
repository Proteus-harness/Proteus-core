//! Icon assets: the component defaults plus the few extra Lucide icons the
//! client uses (`gpui_kit::assets::IconName` names them).

use std::borrow::Cow;

use gpui_kit::assets::Assets as DefaultAssets;
use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        ArrowLeftRight,
        Brain,
        Clock,
        Columns2,
        FolderTree,
        Gauge,
        GitCompare,
        GripVertical,
        Layers,
        ListChecks,
        MessageSquare,
        Package,
        Puzzle,
        Shield,
        Shrink,
        StickyNote,
        Webhook,
        Workflow,
        Wrench
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => DefaultAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = DefaultAssets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
