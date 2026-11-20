//! Subscription labels for TUI surfaces, preserving status-specific plan detail.

use codex_protocol::account::PlanType;
use codex_protocol::auth::KnownPlan;

pub(crate) fn status_plan_label(plan: PlanType) -> &'static str {
    match plan {
        PlanType::Free => "Free",
        PlanType::Go => "Go",
        PlanType::Plus => "Plus",
        PlanType::Pro => "Pro 200",
        PlanType::ProLite => "Pro 100",
        PlanType::ProMax => "Pro 500",
        PlanType::Team | PlanType::SelfServeBusinessUsageBased => "Business",
        PlanType::Business => "Enterprise",
        PlanType::SelfServeBusinessProLite => "Business Premium",
        PlanType::EnterpriseCbpAutomation => "Enterprise (Automation)",
        PlanType::Enterprise | PlanType::Ent26 | PlanType::EnterpriseCbpUsageBased => "Enterprise",
        PlanType::Edu => "Edu",
        PlanType::EduPlus => "Edu Plus",
        PlanType::EduPro => "Edu Pro",
        PlanType::Sci => KnownPlan::Sci.display_name(),
        PlanType::SciPlus => KnownPlan::SciPlus.display_name(),
        PlanType::Unknown => "Unknown",
    }
}
