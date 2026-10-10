use serde_json::json;

use super::*;

#[test]
fn known_tools_name_the_action_and_its_subject() {
    let command = headline(
        "exec_command",
        &json!({"cmd": "ls -la", "max_output_tokens": 400}),
    );
    assert_eq!(command.label, "Команда");
    assert_eq!(command.subject.as_deref(), Some("ls -la"));
    assert_eq!(
        other_args(
            &json!({"cmd": "ls", "max_output_tokens": 400, "skip": null}),
            command.subject_key.as_deref()
        ),
        vec![("max_output_tokens".to_owned(), "400".to_owned())]
    );
    let files = headline(
        "read_many_files",
        &json!({"paths": ["a.rs", "b.rs", "c.rs"]}),
    );
    assert_eq!(files.subject.as_deref(), Some("a.rs +2"));
    assert_eq!(headline("git_status", &json!({})).subject, None);
}

#[test]
fn unknown_tools_keep_their_name_and_find_a_conventional_subject() {
    let hover = headline("lsp_hover", &json!({"line": 3, "file_path": "src/lib.rs"}));
    assert_eq!(hover.label, "lsp_hover");
    assert_eq!(hover.subject.as_deref(), Some("src/lib.rs"));
    let note = headline("memory_store", &json!({"note": "Use nextest", "ttl": 3}));
    assert_eq!(note.subject.as_deref(), Some("Use nextest"));
    assert_eq!(headline("probe", &json!({"body": "a\nb"})).subject, None);
    let multi = headline("exec_command", &json!({"cmd": "cat <<EOF\nhi\nEOF"}));
    assert_eq!(multi.subject.as_deref(), Some("cat <<EOF …"));
}

#[test]
fn patches_list_files_with_line_stats() {
    let patch = "\
*** Begin Patch
*** Add File: a.txt
+one
+two
*** Update File: src/lib.rs
*** Move to: src/main.rs
@@
-old
+new
 context
*** End Patch";
    let files = patch_files(patch);
    assert_eq!(files.len(), 2);
    assert_eq!(
        (files[0].operation, files[0].additions, files[0].deletions),
        (PatchOperation::Add, 2, 0)
    );
    assert_eq!(files[0].body, "+one\n+two");
    assert_eq!(files[1].path, "src/lib.rs → src/main.rs");
    assert_eq!(files[1].operation, PatchOperation::Move);
    assert_eq!(files[1].body, "@@\n-old\n+new\n context");

    let summary = headline(APPLY_PATCH, &json!({ "input": patch }));
    assert_eq!(summary.subject.as_deref(), Some("a.txt +1"));
    assert_eq!(summary.stats, Some((3, 1)));
}

#[test]
fn plans_summarize_progress() {
    let args = json!({"plan": [
        {"step": "Read", "status": "completed"},
        {"step": "Write", "status": "in_progress"},
        {"step": "Test", "status": "pending"}
    ]});
    assert_eq!(
        headline(UPDATE_PLAN, &args).subject.as_deref(),
        Some("1/3 · Write")
    );
    assert_eq!(plan_steps(&args)[2].status, StepStatus::Pending);
}
