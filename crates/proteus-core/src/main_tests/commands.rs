use super::*;
use crate::cli_commands::{PromptReplayCommand, WorkflowReplayCommand};
use proteus_contracts::domain::{ExchangeId, TurnId};

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn cli_identity_matches_the_installed_release_binary() {
    let command = <Cli as clap::CommandFactory>::command();
    assert_eq!(command.get_name(), "proteus");
    assert_eq!(command.get_version(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn list_commands_dispatch_to_their_handlers() {
    assert!(matches!(
        parse_cli_command(&args(&["modules", "list"])).unwrap(),
        CliCommand::ModulesList
    ));
    assert!(matches!(
        parse_cli_command(&args(&["tools", "list"])).unwrap(),
        CliCommand::ToolsList
    ));
}

#[test]
fn inspect_topology_command_parses_default_and_formats() {
    use InspectTopologyFormat::*;
    let cases: &[(&[&str], InspectTopologyFormat)] = &[
        (&["inspect"], Markdown),
        (&["inspect", "topology", "--format", "json"], Json),
        (&["inspect", "topology", "--format=map"], Map),
        (&["inspect", "topology", "--format=runtime"], Runtime),
        (
            &["inspect", "topology", "--format=runtime-mermaid"],
            RuntimeMermaid,
        ),
        (&["inspect", "topology", "--format=mermaid"], Mermaid),
    ];
    for &(arguments, expected) in cases {
        assert_eq!(
            parse_inspect_topology_command(&args(arguments)).expect("parse"),
            Some(expected),
            "{arguments:?}"
        );
    }
    assert!(
        parse_inspect_topology_command(&args(&["doctor"]))
            .expect("parse")
            .is_none()
    );
}

#[test]
fn inspect_plan_command_parses_text_and_json() {
    assert_eq!(
        parse_inspect_plan_command(&args(&["inspect", "plan"]))
            .expect("parse")
            .expect("plan command"),
        InspectPlanFormat::Text
    );
    assert_eq!(
        parse_inspect_plan_command(&args(&["inspect", "plan", "--format=json"]))
            .expect("parse")
            .expect("plan command"),
        InspectPlanFormat::Json
    );
    assert!(
        parse_inspect_plan_command(&args(&["inspect", "topology"]))
            .expect("parse")
            .is_none()
    );
    assert!(
        parse_inspect_topology_command(&args(&["inspect", "plan"]))
            .expect("parse")
            .is_none()
    );
}

#[test]
fn app_server_stdio_command_is_exact() {
    let cli = Cli::try_parse_from(["proteus", "--new-session", "server", "stdio"]).unwrap();
    assert!(cli.new_session);
    assert!(matches!(
        parse_cli_command(&cli.task).unwrap(),
        CliCommand::ServerStdio
    ));
    let prompt = Cli::try_parse_from(["proteus", "inspect this project"]).unwrap();
    assert!(matches!(
        parse_cli_command(&prompt.task).unwrap(),
        CliCommand::Task
    ));
}

#[test]
fn app_server_http_command_parses_defaults_and_bind_options() {
    let default_config = parse_app_server_http_command(&args(&["server", "http"]))
        .expect("parse")
        .expect("http command");
    assert_eq!(default_config.bind.to_string(), "127.0.0.1:8787");
    assert!(!default_config.require_session_token);
    assert!(!default_config.ready_stdout);
    let supervised =
        parse_app_server_http_command(&args(&["server", "http", "--port", "0", "--ready-stdout"]))
            .expect("parse")
            .expect("http command");
    assert_eq!(supervised.bind.port(), 0);
    assert!(supervised.ready_stdout);

    let custom_loopback_config = parse_app_server_http_command(&args(&[
        "server", "http", "--host", "::1", "--port", "9000",
    ]))
    .expect("parse")
    .expect("http command");
    assert_eq!(custom_loopback_config.bind.to_string(), "[::1]:9000");

    let external_token_config = parse_app_server_http_command(&args(&[
        "server", "http", "--host", "0.0.0.0", "--token", "secret",
    ]))
    .expect("parse")
    .expect("http command");
    assert_eq!(external_token_config.bind.to_string(), "0.0.0.0:8787");
    assert!(external_token_config.require_session_token);

    for host in ["0.0.0.0", "::", "192.0.2.1"] {
        let error = parse_app_server_http_command(&args(&["server", "http", "--host", host]))
            .expect_err("non-loopback bind without token must fail");
        assert!(
            error.to_string().contains("requires --token"),
            "unexpected error for {host}: {error}"
        );
    }

    assert!(
        parse_app_server_http_command(&args(&["server", "web"]))
            .expect("parse")
            .is_none()
    );
    assert!(parse_app_server_http_command(&args(&["server", "http", "--bad"])).is_err());
}

#[test]
fn doctor_command_accepts_only_explicit_session_scope() {
    use crate::cli_doctor::SessionScope;
    for (args, all) in [
        (vec!["doctor"], false),
        (vec!["doctor", "--all-sessions"], true),
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let CliCommand::Doctor(scope) = parse_cli_command(&args).expect("doctor command") else {
            panic!("expected doctor");
        };
        assert_eq!(matches!(scope, SessionScope::All), all);
    }
    assert!(parse_cli_command(&args(&["doctor", "--unknown"])).is_err());
}

#[test]
fn eval_report_command_requires_path() {
    assert_eq!(
        parse_eval_report_command(&args(&["eval", "report", ".proteus/events.jsonl"])).unwrap(),
        Some(".proteus/events.jsonl")
    );
    assert!(parse_eval_report_command(&args(&["eval"])).is_err());
    assert!(parse_eval_report_command(&args(&["eval", "report"])).is_err());
    assert_eq!(parse_eval_report_command(&args(&["doctor"])).unwrap(), None);
}

#[test]
fn prompt_replay_command_parses_options_strictly() {
    let exchange_id: ExchangeId = "7c25efa9-81b7-4412-863a-d90e46d2c894"
        .parse()
        .expect("exchange id");
    assert_eq!(
        parse_prompt_replay_command(&args(&[
            "replay",
            "prompt",
            "/tmp/session/journal.jsonl",
            "--exchange-id",
            &exchange_id.to_string(),
            "--allow-hosted-tools",
            "--json"
        ]))
        .expect("parse"),
        Some(PromptReplayCommand {
            source: PathBuf::from("/tmp/session/journal.jsonl"),
            exchange_id: Some(exchange_id),
            allow_hosted_tools: true,
            json: true,
        })
    );
    assert_eq!(
        parse_prompt_replay_command(&args(&["replay", "prompt", "/tmp/session"])).expect("parse"),
        Some(PromptReplayCommand {
            source: PathBuf::from("/tmp/session"),
            exchange_id: None,
            allow_hosted_tools: false,
            json: false,
        })
    );
    assert_eq!(
        parse_prompt_replay_command(&args(&["replay"])).expect("prompt parser"),
        None
    );
    assert!(parse_workflow_replay_command(&args(&["replay"])).is_err());
    assert!(
        parse_prompt_replay_command(&args(&[
            "replay",
            "prompt",
            "/tmp/session",
            "--exchange-id",
            "not-a-uuid"
        ]))
        .is_err()
    );
    assert!(
        parse_prompt_replay_command(&args(&["replay", "prompt", "/tmp/session", "extra"])).is_err()
    );
    assert_eq!(
        parse_prompt_replay_command(&args(&["doctor"])).expect("parse"),
        None
    );
    assert_eq!(
        parse_prompt_replay_command(&args(&["replay", "workflow", "/tmp/session"]))
            .expect("prompt parser ignores workflow replay"),
        None
    );
}

#[test]
fn workflow_replay_command_parses_options_strictly() {
    let turn_id: TurnId = "71a908f9-e7f2-45ce-afbf-4eaf0f4f3bad"
        .parse()
        .expect("turn id");
    assert_eq!(
        parse_workflow_replay_command(&args(&[
            "replay",
            "workflow",
            "/tmp/session/journal.jsonl",
            &format!("--turn-id={turn_id}"),
            "--json"
        ]))
        .expect("parse"),
        Some(WorkflowReplayCommand {
            source: PathBuf::from("/tmp/session/journal.jsonl"),
            turn_id: Some(turn_id),
            json: true,
        })
    );
    assert_eq!(
        parse_workflow_replay_command(&args(&["replay", "workflow", "/tmp/session"]))
            .expect("parse"),
        Some(WorkflowReplayCommand {
            source: PathBuf::from("/tmp/session"),
            turn_id: None,
            json: false,
        })
    );
    assert!(
        parse_workflow_replay_command(&args(&[
            "replay",
            "workflow",
            "/tmp/session",
            "--turn-id",
            "bad-id"
        ]))
        .is_err()
    );
    assert!(
        parse_workflow_replay_command(&args(&["replay", "workflow", "/tmp/session", "extra"]))
            .is_err()
    );
    assert_eq!(
        parse_workflow_replay_command(&args(&["replay", "prompt", "/tmp/session"]))
            .expect("workflow parser ignores prompt replay"),
        None
    );
}

#[test]
fn init_command_defaults_to_coding_profile() {
    assert_eq!(
        parse_init_command(&args(&["init"])).unwrap(),
        Some(InitProfile::Coding)
    );
    assert_eq!(
        parse_init_command(&args(&["init", "safe"])).unwrap(),
        Some(InitProfile::Safe)
    );
    assert_eq!(
        parse_init_command(&args(&["init", "codex"])).unwrap(),
        Some(InitProfile::Codex)
    );
    assert!(parse_init_command(&args(&["init", "bad"])).is_err());
    assert_eq!(parse_init_command(&args(&["doctor"])).unwrap(), None);
}
