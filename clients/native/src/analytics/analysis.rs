//! Turn analysis presentation: step titles, statuses and turn metrics
//! (ported from the web diagnostics).

use proteus_contracts::app_protocol::analysis::*;

pub fn turn_status(status: AppAnalysisTurnStatus) -> &'static str {
    match status {
        AppAnalysisTurnStatus::Unsettled => "Нет завершения",
        AppAnalysisTurnStatus::Success => "Завершён",
        AppAnalysisTurnStatus::Error => "Ошибка",
        AppAnalysisTurnStatus::Canceled => "Отменён",
        AppAnalysisTurnStatus::Timeout => "Таймаут",
    }
}

pub fn step_title(step: &AppAnalysisStep) -> String {
    match &step.data {
        AppAnalysisStepData::Model {
            request, origin, ..
        } => format!(
            "{} · {}",
            request.model.model,
            if matches!(
                origin,
                proteus_contracts::contracts::ModelCallOrigin::Compactor
            ) {
                "сжатие"
            } else {
                "основной запрос"
            }
        ),
        AppAnalysisStepData::Tool { call, .. } => call.name.clone(),
        AppAnalysisStepData::Compaction { .. } => "Сжатие истории".into(),
        AppAnalysisStepData::Hook { trace } => match &trace.input.event {
            proteus_contracts::contracts::HookEvent::TurnStarted { .. } => "Начало хода",
            proteus_contracts::contracts::HookEvent::BeforeModel { .. } => "Перед запросом модели",
            proteus_contracts::contracts::HookEvent::BeforeTool { .. } => "Перед инструментом",
            proteus_contracts::contracts::HookEvent::AfterTool { .. } => "После инструмента",
            proteus_contracts::contracts::HookEvent::BeforeStop { .. } => "Проверка завершения",
            proteus_contracts::contracts::HookEvent::TurnSettled { .. } => "Завершение хода",
        }
        .into(),
    }
}

pub fn step_kind(step: &AppAnalysisStep) -> &'static str {
    match step.data {
        AppAnalysisStepData::Model { .. } => "Модель",
        AppAnalysisStepData::Tool { .. } => "Инструмент",
        AppAnalysisStepData::Compaction { .. } => "Контекст",
        AppAnalysisStepData::Hook { .. } => "Хуки",
    }
}

pub fn step_has_error(step: &AppAnalysisStep) -> bool {
    match &step.data {
        AppAnalysisStepData::Model {
            failure, response, ..
        } => {
            failure.is_some()
                || response.as_ref().is_some_and(|r| {
                    matches!(
                        r.finish_reason,
                        proteus_contracts::model_standard::FinishReason::Error
                    )
                })
        }
        AppAnalysisStepData::Tool { result, .. } => result.as_ref().is_some_and(|r| !r.ok),
        AppAnalysisStepData::Hook { trace } => trace.steps.iter().any(|step| {
            matches!(
                step.outcome,
                proteus_contracts::contracts::HookStepOutcome::Failed { .. }
            )
        }),
        _ => false,
    }
}

pub fn step_status(step: &AppAnalysisStep) -> &'static str {
    if step_has_error(step) {
        return "Ошибка";
    }
    match &step.data {
        AppAnalysisStepData::Model { response: None, .. } => "Нет результата",
        AppAnalysisStepData::Tool { result: None, .. } => "Нет результата",
        AppAnalysisStepData::Compaction { report, .. } if !report.changed => "Без изменений",
        _ => "Готово",
    }
}

pub fn duration(start: i64, end: Option<i64>) -> String {
    match end {
        Some(end) => format!("{:.2} с", (end - start).max(0) as f64 / 1000.0),
        None => "—".into(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnMetrics {
    pub models: usize,
    pub tools: usize,
    pub errors: usize,
    pub missing: usize,
    pub with_usage: usize,
    pub input: u64,
    pub output: u64,
}

pub fn turn_metrics(steps: &[AppAnalysisStep]) -> TurnMetrics {
    let mut metrics = TurnMetrics::default();
    for step in steps {
        metrics.errors += usize::from(step_has_error(step));
        match &step.data {
            AppAnalysisStepData::Model {
                response, failure, ..
            } => {
                metrics.models += 1;
                metrics.missing += usize::from(response.is_none() && failure.is_none());
                if let Some(usage) = response.as_ref().and_then(|r| r.usage.as_ref()) {
                    metrics.with_usage += 1;
                    metrics.input += u64::from(usage.input_tokens);
                    metrics.output += u64::from(usage.output_tokens);
                }
            }
            AppAnalysisStepData::Tool { result, .. } => {
                metrics.tools += 1;
                metrics.missing += usize::from(result.is_none());
            }
            AppAnalysisStepData::Compaction { .. } | AppAnalysisStepData::Hook { .. } => {}
        }
    }
    metrics
}

/// The step shown after loading a turn: the first failure, else the first step.
pub fn default_step(steps: &[AppAnalysisStep]) -> Option<u64> {
    steps
        .iter()
        .find(|step| step_has_error(step))
        .or_else(|| steps.first())
        .map(|step| step.sequence)
}

pub fn message_text(message: &proteus_contracts::model_standard::CanonicalMessage) -> String {
    let text = message.display_text();
    if text.is_empty() {
        crate::analytics::format::pretty(message)
    } else {
        text
    }
}

pub fn compaction_tokens(report: &proteus_contracts::domain::HistoryCompactionReport) -> String {
    use crate::analytics::format::tokens;
    match (report.original_token_estimate, report.output_token_estimate) {
        (Some(before), Some(after)) => {
            format!("{} → {}", tokens(before.into()), tokens(after.into()))
        }
        (Some(before), None) => tokens(before.into()),
        _ => "n/a".into(),
    }
}
