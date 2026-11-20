//! Token and cost summaries for the selected dashboard thread.

use crate::status::format_credit_micros;
use crate::status::format_estimated_usd_micros;
use crate::status::format_tokens_compact;
use crate::status::thread_cost_micros;
use codex_app_server_protocol::ThreadUsage;
use codex_app_server_protocol::ThreadUsageBreakdownGroup;
use codex_app_server_protocol::TokenUsageBreakdown;
use codex_protocol::account::PlanType;
use ratatui::style::Stylize;
use ratatui::text::Line;

#[derive(Default)]
pub(super) struct AgentsOverviewUsage {
    pub(super) tokens: Option<TokenUsageBreakdown>,
    pub(super) estimate: Option<ThreadUsage>,
}

pub(super) fn usage_lines(
    usage: &AgentsOverviewUsage,
    plan_type: Option<PlanType>,
) -> Vec<Line<'static>> {
    let sum = |count: fn(&ThreadUsageBreakdownGroup) -> Option<i64>| {
        let groups = &usage.estimate.as_ref()?.groups;
        if groups.is_empty() {
            return None;
        }
        groups.iter().try_fold(/*init*/ 0_i64, |total, group| {
            count(group)
                .filter(|tokens| *tokens >= 0)
                .map(|tokens| total.saturating_add(tokens))
        })
    };
    let input = usage
        .tokens
        .as_ref()
        .map(|tokens| tokens.input_tokens)
        .or_else(|| sum(|group| group.input_tokens));
    let output = usage
        .tokens
        .as_ref()
        .map(|tokens| tokens.output_tokens)
        .or_else(|| sum(|group| group.output_tokens));
    let mut lines = Vec::new();
    let mut tokens = Vec::new();
    if let Some(input) = input {
        tokens.push(format!("{} in", format_tokens_compact(input)));
    }
    if let Some(output) = output {
        tokens.push(format!("{} out", format_tokens_compact(output)));
    }
    if !tokens.is_empty() {
        lines.push(vec!["Tokens: ".dim(), tokens.join(" · ").into()].into());
    }
    if let Some(estimate) = &usage.estimate {
        let mut values = Vec::new();
        if plan_type != Some(PlanType::Ent26) && estimate.estimated_usage_credits_micros >= 0 {
            values.push(format!(
                "{} credits",
                format_credit_micros(estimate.estimated_usage_credits_micros)
            ));
        }
        if let Some(cost) =
            thread_cost_micros(estimate, plan_type).and_then(format_estimated_usd_micros)
        {
            values.push(cost);
        }
        if !values.is_empty() {
            lines.push(vec!["Est. usage: ".dim(), values.join(" · ").into()].into());
        }
    }
    lines
}
