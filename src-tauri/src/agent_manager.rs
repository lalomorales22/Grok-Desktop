use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::agent::{self, AgentConfig, AgentEvent, AgentLoopResult, ToolCallRecord};
use crate::error::{AppError, AppResult};
use crate::tools::ToolRegistry;
use crate::types::TokenUsage;

// ---------------------------------------------------------------------------
// Agent state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Planning,
    Executing,
    AwaitingTool,
    AwaitingSubagent,
    Reviewing,
    Complete,
    Error,
    Cancelled,
}

impl AgentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Planning => "planning",
            Self::Executing => "executing",
            Self::AwaitingTool => "awaiting_tool",
            Self::AwaitingSubagent => "awaiting_subagent",
            Self::Reviewing => "reviewing",
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

// ---------------------------------------------------------------------------
// Agent instance metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub task_description: String,
    pub model_id: String,
    pub state: AgentState,
    pub iterations: usize,
    pub tool_calls: Vec<ToolCallRecord>,
    pub result_summary: Option<String>,
    pub children: Vec<String>,
}

// ---------------------------------------------------------------------------
// Manager event (forwarded to the frontend)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind")]
pub enum ManagerEvent {
    #[serde(rename = "agent_spawned")]
    AgentSpawned { agent: AgentInfo },
    #[serde(rename = "agent_state_changed")]
    AgentStateChanged { agent_id: String, state: AgentState },
    #[serde(rename = "agent_tool_call")]
    AgentToolCall {
        agent_id: String,
        tool_name: String,
        tool_input: String,
        call_id: String,
    },
    #[serde(rename = "agent_tool_result")]
    AgentToolResult {
        agent_id: String,
        call_id: String,
        tool_name: String,
        success: bool,
        output: String,
    },
    #[serde(rename = "agent_text_delta")]
    AgentTextDelta { agent_id: String, text: String },
    #[serde(rename = "agent_complete")]
    AgentComplete {
        agent_id: String,
        result_summary: String,
        iterations: usize,
    },
    #[serde(rename = "agent_error")]
    AgentError { agent_id: String, message: String },
}

// ---------------------------------------------------------------------------
// Internal agent entry
// ---------------------------------------------------------------------------

struct AgentEntry {
    info: AgentInfo,
    cancel: CancellationToken,
}

// ---------------------------------------------------------------------------
// AgentManager
// ---------------------------------------------------------------------------

/// Manages a tree of agents with parent-child relationships, resource limits,
/// and parallel execution support.
#[derive(Clone)]
pub struct AgentManager {
    agents: Arc<RwLock<HashMap<String, AgentEntry>>>,
    max_concurrent: usize,
    max_total_iterations: usize,
    total_iterations: Arc<Mutex<usize>>,
}

impl AgentManager {
    pub fn new(max_concurrent: usize, max_total_iterations: usize) -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent,
            max_total_iterations,
            total_iterations: Arc::new(Mutex::new(0)),
        }
    }

    /// Current count of non-terminal agents.
    pub async fn active_count(&self) -> usize {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|e| !matches!(e.info.state, AgentState::Complete | AgentState::Error | AgentState::Cancelled))
            .count()
    }

    /// Retrieve info snapshots for all agents in the tree.
    pub async fn list_agents(&self) -> Vec<AgentInfo> {
        let agents = self.agents.read().await;
        agents.values().map(|e| e.info.clone()).collect()
    }

    /// Retrieve a single agent's info.
    pub async fn get_agent(&self, agent_id: &str) -> Option<AgentInfo> {
        let agents = self.agents.read().await;
        agents.get(agent_id).map(|e| e.info.clone())
    }

    /// Register a new agent. Returns its id.
    pub async fn register_agent(
        &self,
        parent_id: Option<String>,
        task_description: String,
        model_id: String,
        cancel: CancellationToken,
    ) -> AppResult<String> {
        if self.active_count().await >= self.max_concurrent {
            return Err(AppError::message(format!(
                "max concurrent agents ({}) reached",
                self.max_concurrent
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();

        let info = AgentInfo {
            id: id.clone(),
            parent_id: parent_id.clone(),
            task_description,
            model_id,
            state: AgentState::Idle,
            iterations: 0,
            tool_calls: Vec::new(),
            result_summary: None,
            children: Vec::new(),
        };

        // Register as child of parent
        let mut agents = self.agents.write().await;
        if let Some(pid) = &parent_id {
            if let Some(parent) = agents.get_mut(pid) {
                parent.info.children.push(id.clone());
            }
        }

        agents.insert(
            id.clone(),
            AgentEntry {
                info,
                cancel,
            },
        );

        Ok(id)
    }

    /// Update agent state.
    pub async fn set_state(&self, agent_id: &str, state: AgentState) {
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.info.state = state;
        }
    }

    /// Record iterations count.
    pub async fn set_iterations(&self, agent_id: &str, iterations: usize) {
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.info.iterations = iterations;
        }
    }

    /// Record a tool call.
    pub async fn add_tool_call(&self, agent_id: &str, record: ToolCallRecord) {
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.info.tool_calls.push(record);
        }
    }

    /// Set result summary on completion.
    pub async fn set_result(&self, agent_id: &str, summary: String) {
        let mut agents = self.agents.write().await;
        if let Some(entry) = agents.get_mut(agent_id) {
            entry.info.result_summary = Some(summary);
        }
    }

    /// Cancel an agent and all its children.
    pub async fn cancel_agent(&self, agent_id: &str) {
        let agents = self.agents.read().await;
        if let Some(entry) = agents.get(agent_id) {
            entry.cancel.cancel();
            // Cancel children recursively
            let children = entry.info.children.clone();
            drop(agents);
            for child_id in &children {
                Box::pin(self.cancel_agent(child_id)).await;
            }
        }
    }

    /// Check if global iteration budget allows more iterations.
    pub async fn try_increment_iterations(&self, count: usize) -> bool {
        let mut total = self.total_iterations.lock().await;
        if *total + count > self.max_total_iterations {
            return false;
        }
        *total += count;
        true
    }

    /// Reset the manager (e.g., between conversations).
    pub async fn reset(&self) {
        let mut agents = self.agents.write().await;
        // Cancel all active agents
        for entry in agents.values() {
            entry.cancel.cancel();
        }
        agents.clear();
        *self.total_iterations.lock().await = 0;
    }

    /// Run a single agent to completion. This is the main entry point for
    /// spawning an agent (either root or sub-agent).
    pub async fn run_agent(
        &self,
        agent_id: &str,
        client: &reqwest::Client,
        api_key: &str,
        config: &AgentConfig,
        history: Vec<Value>,
        tools: &ToolRegistry,
        cancel: CancellationToken,
        on_event: impl Fn(ManagerEvent) -> AppResult<()> + Send + Sync,
    ) -> AppResult<AgentLoopResult> {
        self.set_state(agent_id, AgentState::Executing).await;
        let _ = on_event(ManagerEvent::AgentStateChanged {
            agent_id: agent_id.to_string(),
            state: AgentState::Executing,
        });

        let agent_id_owned = agent_id.to_string();
        let on_event_ref = &on_event;
        let manager_ref = &self;

        let result = agent::run_agent(
            client,
            api_key,
            config,
            history,
            tools,
            cancel.clone(),
            |event| {
                // Forward agent events as manager events
                match &event {
                    AgentEvent::ToolCall { tool_name, tool_input, call_id } => {
                        manager_ref.set_state_sync(&agent_id_owned, AgentState::AwaitingTool);
                        let _ = on_event_ref(ManagerEvent::AgentToolCall {
                            agent_id: agent_id_owned.clone(),
                            tool_name: tool_name.clone(),
                            tool_input: tool_input.clone(),
                            call_id: call_id.clone(),
                        });
                    }
                    AgentEvent::ToolResult { call_id, tool_name, success, output } => {
                        manager_ref.set_state_sync(&agent_id_owned, AgentState::Executing);
                        let _ = on_event_ref(ManagerEvent::AgentToolResult {
                            agent_id: agent_id_owned.clone(),
                            call_id: call_id.clone(),
                            tool_name: tool_name.clone(),
                            success: *success,
                            output: output.clone(),
                        });
                    }
                    AgentEvent::TextDelta { text } => {
                        let _ = on_event_ref(ManagerEvent::AgentTextDelta {
                            agent_id: agent_id_owned.clone(),
                            text: text.clone(),
                        });
                    }
                    AgentEvent::Complete { text, iterations } => {
                        let _ = on_event_ref(ManagerEvent::AgentComplete {
                            agent_id: agent_id_owned.clone(),
                            result_summary: text.clone(),
                            iterations: *iterations,
                        });
                    }
                    AgentEvent::Error { message } => {
                        let _ = on_event_ref(ManagerEvent::AgentError {
                            agent_id: agent_id_owned.clone(),
                            message: message.clone(),
                        });
                    }
                    _ => {}
                }
                Ok(())
            },
        )
        .await;

        match &result {
            Ok(r) => {
                self.set_state(agent_id, AgentState::Complete).await;
                self.set_iterations(agent_id, r.iterations).await;
                self.set_result(agent_id, truncate_summary(&r.final_text, 500)).await;
                for record in &r.tool_calls_made {
                    self.add_tool_call(agent_id, record.clone()).await;
                }
            }
            Err(e) if e.to_string() == "cancelled" => {
                self.set_state(agent_id, AgentState::Cancelled).await;
            }
            Err(_) => {
                self.set_state(agent_id, AgentState::Error).await;
            }
        }

        result
    }

    /// Synchronous state setter for use inside the non-async on_event callback.
    /// Uses a blocking write — acceptable because the lock is never held long.
    fn set_state_sync(&self, _agent_id: &str, _state: AgentState) {
        // This is a no-op for now since we can't do async inside the sync callback.
        // State updates happen in the async `run_agent` wrapper instead.
    }
}

fn truncate_summary(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
