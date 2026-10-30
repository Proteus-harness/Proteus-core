use super::*;

fn profile_with_reasoning(reasoning: ReasoningConfig) -> AppConfig {
    let mut config = crate::test_model::config();
    config.providers.get_mut("fake").unwrap().reasoning = reasoning;
    config
}

fn prepare(config: AppConfig, cwd: &std::path::Path) -> PreparedAssembly {
    let catalog = crate::test_support::module_catalog_for(&config);
    PreparedAssembly::from_catalog(config, cwd.to_path_buf(), None, catalog)
        .expect("prepared assembly")
}

#[tokio::test]
async fn reasoning_reenable_uses_published_defaults_without_changing_admitted_settings() {
    let cwd = tempfile::tempdir().expect("workspace");
    let first = ReasoningConfig::new(Some("low".into()), true).with_budget_tokens(Some(1024));
    let next = ReasoningConfig::new(Some("high".into()), false).with_budget_tokens(Some(8192));
    let config = profile_with_reasoning(first.clone());
    let catalog = crate::test_support::module_catalog_for(&config);
    let runtime = AgentRuntime::builder(config, cwd.path().to_path_buf())
        .with_module_catalog(catalog)
        .build()
        .expect("runtime");
    let admitted = runtime.capture_execution_snapshot().await;

    runtime
        .reload_assembly_with_commit(
            prepare(profile_with_reasoning(next.clone()), cwd.path()),
            None,
            None,
            None,
            Some(next.clone()),
            || async { Ok(()) },
        )
        .await
        .expect("publish next profile");
    assert_eq!(runtime.reasoning().await, next);
    runtime.set_reasoning_enabled(false).await;
    assert_eq!(runtime.reasoning().await, ReasoningConfig::default());
    runtime.set_reasoning_enabled(true).await;
    assert_eq!(runtime.reasoning().await, next);

    runtime
        .set_reasoning_effort(Some("none".into()))
        .await
        .expect("disable by effort");
    runtime
        .set_reasoning_effort(Some("medium".into()))
        .await
        .expect("select explicit effort");
    let overridden =
        ReasoningConfig::new(Some("medium".into()), false).with_budget_tokens(Some(8192));
    assert_eq!(runtime.reasoning().await, overridden);

    // Reloading another field must not erase the session override; defaults
    // nevertheless remain those of the bound profile for a later re-enable.
    let mut unrelated = profile_with_reasoning(next.clone());
    unrelated.profile.name = "renamed".into();
    runtime
        .reload_assembly(prepare(unrelated, cwd.path()), None)
        .await
        .expect("reload unrelated setting");
    assert_eq!(runtime.reasoning().await, overridden);
    runtime.set_reasoning_enabled(false).await;
    runtime.set_reasoning_enabled(true).await;
    assert_eq!(runtime.reasoning().await, next);

    assert_eq!(admitted.reasoning, first);
    assert_eq!(admitted.runtime.epoch.as_u64(), 0);
    assert_eq!(runtime.module_epoch().await.as_u64(), 2);
    assert_eq!(
        admitted
            .runtime
            .registry
            .model_config
            .as_ref()
            .unwrap()
            .reasoning,
        first
    );
}

#[tokio::test]
async fn failed_profile_commit_keeps_reasoning_defaults_until_successful_publication() {
    let cwd = tempfile::tempdir().expect("workspace");
    let first = ReasoningConfig::new(Some("low".into()), true).with_budget_tokens(Some(1024));
    let next = ReasoningConfig::new(Some("high".into()), false).with_budget_tokens(Some(8192));
    let config = profile_with_reasoning(first.clone());
    let catalog = crate::test_support::module_catalog_for(&config);
    let runtime = AgentRuntime::builder(config, cwd.path().to_path_buf())
        .with_module_catalog(catalog)
        .build()
        .expect("runtime");

    let error = runtime
        .reload_assembly_with_commit(
            prepare(profile_with_reasoning(next.clone()), cwd.path()),
            None,
            None,
            None,
            Some(next),
            || async { anyhow::bail!("persistence refused") },
        )
        .await
        .expect_err("failed commit must not publish");
    assert!(error.to_string().contains("persistence refused"));
    assert_eq!(runtime.module_epoch().await.as_u64(), 0);
    assert_eq!(runtime.reasoning().await, first);
    runtime.set_reasoning_enabled(false).await;
    runtime.set_reasoning_enabled(true).await;
    assert_eq!(runtime.reasoning().await, first);

    // Removing the configured model must not retain the first provider's
    // defaults through a second, constructor-owned source of truth.
    let mut model_free = AppConfig::default();
    model_free.active_provider = None;
    model_free.providers.clear();
    runtime
        .reload_assembly(prepare(model_free, cwd.path()), None)
        .await
        .expect("publish model-free assembly");
    runtime.set_reasoning_enabled(false).await;
    runtime.set_reasoning_enabled(true).await;
    assert_eq!(runtime.reasoning().await, ReasoningConfig::default());
    assert!(runtime.model_ref().await.is_none());
}
