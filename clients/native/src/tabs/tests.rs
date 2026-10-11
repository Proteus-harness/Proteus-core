use std::path::Path;

use super::{TabLayout, chat, chat_dir, split_document};

const A: &str = "chat:/s/a";
const B: &str = "chat:/s/b";

fn tabs(layout: &TabLayout) -> Vec<(Vec<&str>, Option<&str>)> {
    layout
        .groups
        .iter()
        .map(|group| {
            (
                group.tabs.iter().map(String::as_str).collect(),
                group.active.as_deref(),
            )
        })
        .collect()
}

fn with_chat() -> TabLayout {
    let mut layout = TabLayout::default();
    layout.open(A, false);
    layout
}

#[test]
fn ids_tell_chats_from_documents() {
    assert_eq!(chat(Path::new("/s/a")), A);
    assert_eq!(chat_dir(A), Some(Path::new("/s/a")));
    assert_eq!(chat_dir("files:src/a.rs"), None);
    assert_eq!(split_document(A), None);
    assert_eq!(
        split_document("files:src/a:b.rs"),
        Some(("files", "src/a:b.rs"))
    );
}

#[test]
fn opening_avoids_covering_a_chat() {
    let mut layout = with_chat();
    layout.open("files", false);
    assert_eq!(tabs(&layout), [(vec![A, "files"], Some("files"))]);

    let mut layout = with_chat();
    layout.open("files", true);
    assert_eq!(
        tabs(&layout),
        [(vec![A], Some(A)), (vec!["files"], Some("files"))]
    );
    assert_eq!(layout.focused, 1);

    // Opening an open tab only shows it; the chat group stays as it is.
    layout.focus(0);
    layout.open("notes", true);
    layout.open("files", true);
    assert_eq!(layout.active(1), Some("files"));
    assert_eq!(layout.groups[1].tabs, ["files", "notes"]);

    // Another chat joins the chat in view, even from the documents group.
    layout.open(B, true);
    assert_eq!(layout.groups[0].tabs, [A, B]);
    assert_eq!(layout.active(0), Some(B));
    assert!(layout.validate().is_ok());
}

#[test]
fn closing_falls_back_and_merges_empty_groups() {
    let mut layout = with_chat();
    layout.open("files", false);
    layout.open("notes", false);
    layout.activate(0, "files");
    layout.activate(0, "notes");
    layout.close("notes");
    assert_eq!(layout.active(0), Some("files"));

    layout.transfer(0);
    assert_eq!(
        tabs(&layout),
        [(vec![A], Some(A)), (vec!["files"], Some("files"))]
    );
    layout.close("files");
    assert_eq!(tabs(&layout), [(vec![A], Some(A))]);
    assert_eq!(layout.focused, 0);

    // The last chat closes too, leaving an empty area.
    layout.close(A);
    assert_eq!(tabs(&layout), [(vec![], None)]);
    assert!(layout.validate().is_ok());
    assert_eq!(layout, TabLayout::default());
}

#[test]
fn moving_reorders_and_splits() {
    let mut layout = with_chat();
    layout.open("files", false);
    layout.open("notes", false);
    layout.move_tab("notes", 0, Some(A));
    assert_eq!(layout.groups[0].tabs, ["notes", A, "files"]);

    layout.move_tab(A, 1, None);
    assert_eq!(
        tabs(&layout),
        [(vec!["notes", "files"], Some("notes")), (vec![A], Some(A))]
    );
    // A third group is not possible; dropping the last tab merges back.
    layout.move_tab("files", 2, None);
    assert_eq!(layout.groups.len(), 2);
    layout.move_tab(A, 0, Some("files"));
    assert_eq!(tabs(&layout), [(vec!["notes", A, "files"], Some(A))]);
}

#[test]
fn split_toggles_and_validation_rejects_broken_layouts() {
    let mut layout = with_chat();
    layout.open("files", false);
    layout.toggle_split();
    assert_eq!(tabs(&layout)[1], (vec![], None));
    assert!(layout.validate().is_ok());
    layout.toggle_split();
    assert_eq!(tabs(&layout), [(vec![A, "files"], Some("files"))]);

    let mut broken = with_chat();
    broken.groups[0].tabs.push(A.into());
    assert!(broken.validate().is_err());
    let mut broken = with_chat();
    broken.groups[0].active = Some("files".into());
    assert!(broken.validate().is_err());
    let mut broken = with_chat();
    broken.groups[0].active = None;
    assert!(broken.validate().is_err());
}

#[test]
fn retain_drops_gone_chats_and_documents() {
    let mut layout = with_chat();
    layout.open(B, false);
    layout.open("files", false);
    layout.retain(|id| id != B && id != "files");
    assert_eq!(tabs(&layout), [(vec![A], Some(A))]);
    layout.retain(|_| false);
    assert_eq!(layout, TabLayout::default());
}

#[test]
fn active_chat_follows_focus_and_survives_hiding() {
    let mut layout = TabLayout::default();
    assert_eq!(layout.active_chat(None), None);
    layout.open(A, true);
    layout.open("files", true);
    layout.open(B, true);
    assert_eq!(tabs(&layout)[0], (vec![A, B], Some(B)));
    assert_eq!(layout.focused, 0);
    assert_eq!(layout.active_chat(Some(A)).as_deref(), Some(B));

    // A document in focus keeps the last chat; a closed one is replaced by
    // a chat in view.
    layout.focus(1);
    assert_eq!(layout.active_chat(Some(A)).as_deref(), Some(A));
    layout.close(A);
    assert_eq!(layout.active_chat(Some(A)).as_deref(), Some(B));

    // Hidden behind a document, a chat still counts when nothing else does.
    layout.move_tab("files", 0, None);
    assert_eq!(tabs(&layout), [(vec![B, "files"], Some("files"))]);
    assert_eq!(layout.chats().collect::<Vec<_>>(), [B]);
    assert_eq!(layout.active_chat(None).as_deref(), Some(B));
}

#[test]
fn preview_tab_is_replaced_until_kept() {
    let mut layout = with_chat();
    layout.open_preview("files:a.rs", false);
    layout.open_preview("files:b.rs", false);
    assert_eq!(tabs(&layout), [(vec![A, "files:b.rs"], Some("files:b.rs"))]);
    assert_eq!(layout.preview.as_deref(), Some("files:b.rs"));

    // A kept tab stays; the next preview opens next to it.
    layout.keep("files:b.rs");
    layout.open_preview("files:c.rs", false);
    layout.open_preview("files:b.rs", false);
    assert_eq!(layout.groups[0].tabs, [A, "files:b.rs", "files:c.rs"]);
    assert_eq!(layout.active(0), Some("files:b.rs"));
    assert_eq!(layout.preview.as_deref(), Some("files:c.rs"));

    // Closing the preview tab forgets it.
    layout.close("files:c.rs");
    assert_eq!(layout.preview, None);
    assert!(layout.validate().is_ok());

    // Chats preview the same way.
    layout.open_preview(B, false);
    layout.open_preview("chat:/s/c", false);
    assert_eq!(layout.groups[0].tabs, [A, "files:b.rs", "chat:/s/c"]);
    layout.keep("chat:/s/c");
    assert_eq!(layout.preview, None);
}
