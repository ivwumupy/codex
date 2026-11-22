//! Non-command tool lifecycle rendering for `ChatWidget`.
//!
//! This module handles patch, MCP, web search, image, and collaborator tool
//! events as transcript cells.

use super::*;
use crate::thread_transcript::tools::McpHistory;
use codex_utils_path_uri::LegacyAppPathString;

/// Build the same live tool row for a focused or parked conversation.
pub(crate) fn running_tool_cell(
    item: &ThreadItemPayload,
    cwd: &Path,
) -> Option<Box<dyn HistoryCell>> {
    let label = match item {
        ThreadItemPayload::CommandExecution {
            command, source, ..
        } if *source != ExecCommandSource::UserShell => {
            format!(
                "Running {}",
                strip_bash_lc_and_escape(&split_command_string(command))
            )
        }
        ThreadItemPayload::FileChange { changes, .. } => {
            let summary = crate::diff_render::running_patch_summary(
                &file_update_changes_to_display(changes.clone()),
                cwd,
            );
            return Some(Box::new(PlainHistoryCell::new(vec![summary])));
        }
        ThreadItemPayload::McpToolCall { server, tool, .. } => format!("Calling {server}.{tool}"),
        ThreadItemPayload::DynamicToolCall {
            namespace, tool, ..
        } => match namespace {
            Some(namespace) => format!("Calling {namespace}.{tool}"),
            None => format!("Calling {tool}"),
        },
        ThreadItemPayload::CollabAgentToolCall { tool, .. } => match tool {
            CollabAgentTool::SpawnAgent => "Starting agent",
            CollabAgentTool::SendInput | CollabAgentTool::SendMessage => "Sending message to agent",
            CollabAgentTool::FollowupTask => "Sending follow-up task",
            CollabAgentTool::InterruptAgent => "Interrupting agent",
            CollabAgentTool::ListAgents => "Listing agents",
            CollabAgentTool::ResumeAgent => "Resuming agent",
            CollabAgentTool::Wait => "Waiting for agents",
            CollabAgentTool::CloseAgent => "Closing agent",
        }
        .into(),
        ThreadItemPayload::WebSearch(_) => "Searching the web".into(),
        ThreadItemPayload::ImageGeneration(_) => "Generating image".into(),
        ThreadItemPayload::ImageView { .. } => "Viewing image".into(),
        ThreadItemPayload::Sleep(_) => "Sleeping".into(),
        _ => return None,
    };
    let label = label.lines().next().unwrap_or_default().to_owned();
    Some(Box::new(PlainHistoryCell::new(vec![Line::from(vec![
        "• ".dim(),
        label.into(),
    ])])))
}

pub(crate) fn upsert_running_tool(
    tools: &mut Vec<(String, Box<dyn HistoryCell>)>,
    id: &str,
    cell: Box<dyn HistoryCell>,
) {
    if let Some((_, current)) = tools.iter_mut().find(|(key, _)| key == id) {
        *current = cell;
    } else {
        tools.push((id.to_owned(), cell));
    }
}

impl ChatWidget {
    pub(super) fn show_running_tool(&mut self, item: &ThreadItemPayload) {
        if let Some(cell) = running_tool_cell(item, self.config.cwd.as_path()) {
            upsert_running_tool(&mut self.transcript.running_tools, item.id(), cell);
            self.bump_active_cell_revision();
            self.request_redraw();
        }
    }

    pub(super) fn hide_running_tool(&mut self, id: &str) {
        if let Some(index) = self
            .transcript
            .running_tools
            .iter()
            .position(|(key, _)| key == id)
        {
            self.transcript.running_tools.remove(index);
            self.bump_active_cell_revision();
            self.request_redraw();
        }
    }

    pub(super) fn on_view_image_tool_call(&mut self, id: String, path: LegacyAppPathString) {
        self.hide_running_tool(&id);
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_view_image_tool_call(id, path));
        self.request_redraw();
    }

    pub(super) fn on_image_generation_begin(&mut self) {
        self.flush_answer_stream_with_separator();
        if self.bottom_pane.is_task_running() {
            self.bottom_pane.ensure_status_indicator();
        }
    }

    pub(super) fn on_image_generation_end(
        &mut self,
        call_id: String,
        status: String,
        revised_prompt: Option<String>,
        saved_path: Option<AbsolutePathBuf>,
    ) {
        self.hide_running_tool(&call_id);
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_image_generation_call(
            call_id,
            &status,
            revised_prompt,
            saved_path,
        ));
        self.request_redraw();
    }

    pub(super) fn on_file_change_completed(&mut self, item: ThreadItemPayload) {
        self.hide_running_tool(item.id());
        self.defer_or_handle(
            item,
            InterruptManager::push_item_completed,
            Self::handle_file_change_completed_now,
        );
    }

    pub(super) fn on_mcp_tool_call_started(&mut self, item: ThreadItemPayload) {
        self.show_running_tool(&item);
        self.defer_or_handle(
            item,
            InterruptManager::push_item_started,
            Self::handle_mcp_tool_call_started_now,
        );
    }

    pub(super) fn on_mcp_tool_call_completed(&mut self, item: ThreadItemPayload) {
        self.hide_running_tool(item.id());
        self.defer_or_handle(
            item,
            InterruptManager::push_item_completed,
            Self::handle_mcp_tool_call_completed_now,
        );
    }

    pub(super) fn on_web_search_begin(&mut self, call_id: String) {
        self.flush_answer_stream_with_separator();
        self.flush_active_cell();
        self.transcript.active_cell = Some(Box::new(history_cell::new_active_web_search_call(
            call_id,
            String::new(),
            self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(super) fn on_web_search_end(
        &mut self,
        call_id: String,
        query: String,
        action: codex_app_server_protocol::WebSearchAction,
    ) {
        self.hide_running_tool(&call_id);
        self.flush_answer_stream_with_separator();
        let mut handled = false;
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<WebSearchCell>())
            && cell.call_id() == call_id
        {
            cell.update(action.clone(), query.clone());
            cell.complete();
            self.bump_active_cell_revision();
            self.flush_active_cell();
            handled = true;
        }

        if !handled {
            self.add_to_history(history_cell::new_web_search_call(call_id, query, action));
        }
    }

    pub(super) fn on_collab_event(&mut self, cell: impl HistoryCell + 'static) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(cell);
        self.request_redraw();
    }

    pub(super) fn on_collab_agent_tool_call(&mut self, item: ThreadItemPayload) {
        let ThreadItemPayload::CollabAgentToolCall {
            id, tool, status, ..
        } = &item
        else {
            return;
        };
        if matches!(status, CollabAgentToolCallStatus::InProgress) {
            self.show_running_tool(&item);
        } else {
            self.hide_running_tool(id);
        }
        if matches!(tool, CollabAgentTool::SpawnAgent)
            && let Some(spawn_request) = multi_agents::spawn_request_summary(&item)
        {
            self.pending_collab_spawn_requests
                .insert(id.clone(), spawn_request);
        }

        let cached_spawn_request = if matches!(tool, CollabAgentTool::SpawnAgent)
            && !matches!(status, CollabAgentToolCallStatus::InProgress)
        {
            self.pending_collab_spawn_requests.remove(id)
        } else {
            None
        };

        if let Some(cell) = multi_agents::tool_call_history_cell(
            &item,
            cached_spawn_request.as_ref(),
            |thread_id| self.collab_agent_metadata(thread_id),
        ) {
            self.on_collab_event(cell);
        }
    }

    pub(super) fn on_sub_agent_activity(&mut self, item: ThreadItemPayload) {
        // Background agents can finish while the parent answer is still streaming.
        // Keep that stream intact until its authoritative message completion.
        // After the turn stops, leftover prompts must not hold up late activity.
        if !self.turn_lifecycle.agent_turn_running && self.stream_controller.is_none() {
            self.handle_sub_agent_activity_now(item);
        } else {
            self.defer_or_handle(
                item,
                InterruptManager::push_item_completed,
                Self::handle_sub_agent_activity_now,
            );
        }
    }

    fn handle_sub_agent_activity_now(&mut self, item: ThreadItemPayload) {
        if let Some(cell) = multi_agents::sub_agent_activity_history_cell(&item) {
            self.on_collab_event(cell);
        }
    }

    pub(crate) fn handle_file_change_completed_now(&mut self, item: ThreadItemPayload) {
        let ThreadItemPayload::FileChange {
            id,
            changes,
            status,
        } = item
        else {
            return;
        };
        use codex_app_server_protocol::PatchApplyStatus;
        match status {
            PatchApplyStatus::Completed if !changes.is_empty() => {
                self.add_to_history(
                    history_cell::new_patch_event(
                        file_update_changes_to_display(changes),
                        &self.config.cwd,
                    )
                    .with_activity_id(id),
                );
            }
            PatchApplyStatus::Failed => {
                self.add_to_history(history_cell::new_patch_apply_failure(String::new()));
            }
            _ => {}
        }
    }

    pub(crate) fn handle_mcp_tool_call_started_now(&mut self, item: ThreadItemPayload) {
        let ThreadItemPayload::McpToolCall {
            id,
            server,
            tool,
            arguments,
            ..
        } = item
        else {
            return;
        };
        self.flush_answer_stream_with_separator();
        let invocation = McpInvocation {
            server,
            tool,
            arguments: Some(arguments),
        };
        if invocation.is_computer_activity() {
            let call = history_cell::new_active_mcp_tool_call(
                id,
                invocation,
                self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
            );
            self.update_computer_activity(|cell| cell.start(call));
            self.bump_active_cell_revision();
            self.request_redraw();
            return;
        }
        self.flush_active_cell();
        self.transcript.active_cell = Some(Box::new(history_cell::new_active_mcp_tool_call(
            id,
            invocation,
            self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
        )));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    pub(crate) fn handle_mcp_tool_call_completed_now(&mut self, item: ThreadItemPayload) {
        self.flush_answer_stream_with_separator();

        let Some(McpHistory {
            id,
            invocation,
            duration,
            result,
        }) = McpHistory::from_item(item)
        else {
            return;
        };

        if invocation.is_computer_activity() {
            let call = history_cell::new_active_mcp_tool_call(
                id,
                invocation,
                self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
            );
            self.update_computer_activity(|cell| cell.complete(call, duration, result));
            self.bump_active_cell_revision();
            self.request_redraw();
            return;
        }

        match self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<McpToolCallCell>())
        {
            Some(cell) if cell.call_id() == id => cell.complete(duration, result),
            _ => {
                self.flush_active_cell();
                let mut cell = history_cell::new_active_mcp_tool_call(
                    id,
                    invocation,
                    self.local_settings.tui.animations && self.local_settings.tui.effects.progress,
                );
                cell.complete(duration, result);
                self.transcript.active_cell = Some(Box::new(cell));
            }
        };

        self.flush_active_cell();
    }

    /// Preserve pending calls and timers, returning the revisions before and after hydration.
    pub(crate) fn prepend_active_computer_history(
        &mut self,
        older: &dyn HistoryCell,
        turns: &[Turn],
    ) -> Option<(u64, u64)> {
        let previous_revision = self.transcript.active_cell_revision;
        let active = self.transcript.active_cell.as_mut().and_then(|cell| {
            cell.as_any_mut()
                .downcast_mut::<history_cell::ComputerActivityCell>()
        })?;
        let older = crate::thread_transcript::older_computer_group(older, active, turns)?;
        active.prepend(older);
        self.bump_active_cell_revision();
        Some((previous_revision, self.transcript.active_cell_revision))
    }

    /// Reuse only adjacent computer calls; all other active cells form a transcript boundary.
    fn update_computer_activity(
        &mut self,
        update: impl FnOnce(&mut history_cell::ComputerActivityCell),
    ) {
        if let Some(cell) = self.transcript.active_cell.as_mut().and_then(|cell| {
            cell.as_any_mut()
                .downcast_mut::<history_cell::ComputerActivityCell>()
        }) {
            update(cell);
        } else {
            self.flush_active_cell();
            let mut cell = history_cell::ComputerActivityCell::default();
            update(&mut cell);
            self.transcript.active_cell = Some(Box::new(cell));
        }
    }

    pub(crate) fn handle_queued_item_started_now(&mut self, item: ThreadItemPayload) {
        match item {
            item @ ThreadItemPayload::CommandExecution { .. } => {
                self.handle_command_execution_started_now(item);
            }
            item @ ThreadItemPayload::McpToolCall { .. } => {
                self.handle_mcp_tool_call_started_now(item);
            }
            item @ ThreadItemPayload::DynamicToolCall { .. } => {
                self.handle_dynamic_tool_item_now(item)
            }
            _ => {}
        }
    }

    pub(crate) fn handle_queued_item_completed_now(&mut self, item: ThreadItemPayload) {
        match item {
            item @ ThreadItemPayload::CommandExecution { .. } => {
                self.handle_command_execution_completed_now(item);
            }
            item @ ThreadItemPayload::FileChange { .. } => {
                self.handle_file_change_completed_now(item)
            }
            item @ ThreadItemPayload::McpToolCall { .. } => {
                self.handle_mcp_tool_call_completed_now(item)
            }
            item @ ThreadItemPayload::DynamicToolCall { .. } => {
                self.handle_dynamic_tool_item_now(item)
            }
            item @ ThreadItemPayload::SubAgentActivity { .. } => {
                self.handle_sub_agent_activity_now(item)
            }
            _ => {}
        }
    }
}
