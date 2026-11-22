//! Session-only model selections leave durable defaults untouched.

use super::*;
use crate::bottom_pane::SelectionSecondaryAction;
use crate::keymap::ListAction;
use crate::model_catalog::LUNA_RESERVE_MODEL;

impl ChatWidget {
    pub(super) fn session_model_selection_action(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) -> Option<SelectionSecondaryAction> {
        // Reserve selections are already session-only.
        if model == LUNA_RESERVE_MODEL {
            return None;
        }
        let key = key_hint::plain(KeyCode::Char('s'));
        let keymap = self.bottom_pane.list_keymap();
        let mut hints = Vec::new();
        if let Some(accept) = keymap.primary_hint(ListAction::Accept) {
            let label = if effort == Some(ReasoningEffortConfig::Ultra) {
                " apply · "
            } else {
                " default · "
            };
            hints.extend([accept.into(), label.dim()]);
        }
        hints.extend([key.into(), " session".dim()]);
        if let Some(cancel) = keymap.primary_hint(ListAction::Cancel) {
            hints.extend([" · ".dim(), cancel.into(), " back".dim()]);
        }
        let warning = effort
            .as_ref()
            .and_then(|effort| self.ultra_reasoning_concurrency_warning(effort));
        let scoped = self.scope_model_picker_event();
        Some(SelectionSecondaryAction {
            key,
            footer_hint: hints.into(),
            action: Box::new(move |tx| {
                tx.send(scoped(AppEvent::SelectSessionModel {
                    model: model.clone(),
                    effort: effort.clone(),
                    service_tier: None,
                }));
                if let Some(warning) = warning.clone() {
                    tx.send(scoped(AppEvent::InsertHistoryCell(Box::new(
                        history_cell::new_warning_event(warning),
                    ))));
                }
            }),
        })
    }
}
