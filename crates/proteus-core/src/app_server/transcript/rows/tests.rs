use super::super::*;
use serde_json::json;

fn hosted(id: &str) -> HostedToolActivity {
    HostedToolActivity::WebSearch {
        id: id.into(),
        status: HostedToolStatus::Completed,
        action: crate::domain::WebSearchAction::Search {
            queries: vec![],
            sources: vec![],
        },
    }
}

fn cite(rows: &mut TranscriptRows) {
    append_citation_metadata(
        &mut rows.rows,
        &Citation::Url {
            start_index: 0,
            end_index: 1,
            title: "source".into(),
            url: "https://example.com".into(),
        },
    );
}

#[test]
fn delayed_results_update_their_cards_across_interleaved_text() {
    let mut rows = TranscriptRows::default();
    for index in 0..128 {
        append_transcript_tool_call(
            &mut rows,
            &ToolCall::new(format!("call-{index}"), "probe", json!({"index": index})),
        );
        rows.rows.last_mut().unwrap().timestamp_ms = Some(index);
        append_transcript_message(
            &mut rows,
            &CanonicalMessage::text(MessageRole::Assistant, format!("between {index}")),
        );
    }
    for index in 0..128 {
        append_transcript_tool_result(
            &mut rows,
            &ToolResult::ok(format!("call-{index}"), format!("result {index}")),
        );
    }
    assert_eq!(rows.rows.len(), 256);
    for index in 0..128 {
        let card = &rows.rows[index * 2];
        assert_eq!(card.timestamp_ms, Some(index as i64));
        let tool = card.tool.as_ref().unwrap();
        assert_eq!(tool.call_id, format!("call-{index}"));
        assert_eq!(tool.result, Some(format!("result {index}")));
        assert_eq!(tool.status, "done");
        assert_eq!(rows.rows[index * 2 + 1].text, format!("between {index}"));
    }
}

#[test]
fn duplicate_hosted_ids_target_the_last_card_and_metadata_still_controls_citations() {
    let mut rows = TranscriptRows::default();
    append_hosted_tool_activity(&mut rows, &hosted("old"));
    append_transcript_tool_call(&mut rows, &ToolCall::new("same", "probe", json!({})));
    append_hosted_tool_activity(&mut rows, &hosted("same"));
    append_transcript_tool_result(
        &mut rows,
        &ToolResult::ok("same".into(), "latest")
            .with_metadata(json!({"provider_hosted": true, "citations": []})),
    );
    cite(&mut rows);
    assert_eq!(rows.rows[1].tool.as_ref().unwrap().status, "running");
    assert_eq!(
        rows.rows[2].tool.as_ref().unwrap().result.as_deref(),
        Some("latest")
    );
    assert_eq!(
        rows.rows[2].tool.as_ref().unwrap().metadata["citations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A matching last hosted card without an array consumes the search; do not
    // retry an older card. Result metadata replaces, rather than merges.
    append_transcript_tool_result(
        &mut rows,
        &ToolResult::ok("same".into(), "no array")
            .with_metadata(json!({"provider_hosted": true, "citations": null})),
    );
    cite(&mut rows);
    assert_eq!(
        rows.rows[0].tool.as_ref().unwrap().metadata["citations"],
        json!([])
    );
    assert_eq!(
        rows.rows[2].tool.as_ref().unwrap().metadata["citations"],
        Value::Null
    );

    append_transcript_tool_result(&mut rows, &ToolResult::ok("same".into(), "not hosted"));
    cite(&mut rows);
    assert_eq!(
        rows.rows[0].tool.as_ref().unwrap().metadata["citations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(rows.rows[2].tool.as_ref().unwrap().metadata, Value::Null);
}

#[test]
fn reset_discards_lookup_and_standalone_results_become_indexed() {
    let mut rows = TranscriptRows::default();
    append_transcript_tool_call(&mut rows, &ToolCall::new("erased", "probe", json!({})));
    rows.clear();
    append_transcript_message(
        &mut rows,
        &CanonicalMessage::text(MessageRole::User, "retained"),
    );
    append_transcript_tool_result(&mut rows, &ToolResult::ok("erased".into(), "first"));
    append_transcript_tool_result(&mut rows, &ToolResult::ok("erased".into(), "second"));
    assert_eq!(rows.rows.len(), 2);
    assert_eq!(rows.rows[0].text, "retained");
    let tool = rows.rows[1].tool.as_ref().unwrap();
    assert_eq!(tool.name, "tool");
    assert_eq!(tool.result.as_deref(), Some("second"));
}
