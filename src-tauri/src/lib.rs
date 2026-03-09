mod agent;
mod agent_manager;
mod db;
mod editor;
mod error;
mod hands;
mod keychain;
mod providers;
mod terminal;
mod tools;
mod types;
mod window;
mod workspace;

use std::sync::{Arc, Mutex};

use base64::Engine;
use db::Database;
use error::AppError;
use hands::HandsService;
use keychain::{MigratingSecretStore, SecretStore};
use providers::ProviderService;
use tauri::{AppHandle, Emitter, Manager, State};
use terminal::{
    ensure_terminal, resize_terminal, terminate_terminal, write_input, TerminalRegistry,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use types::{
    AgentChatRequest, ChatRequest, Conversation, ConversationDetail, ConversationSummary,
    ExportEditorTimelineRequest, GenerateImageRequest, GenerateVideoRequest, HandsStatus,
    ImportLocalMediaRequest, MediaAsset, MediaCategory, ModelDescriptor, NewConversation,
    NewMediaCategory, NewWorkspace, ProviderId, ProviderStatus, RealtimeSession,
    RealtimeSessionRequest, RenameConversation, Settings, SettingsPatch, StreamEvent, StreamHandle,
    TerminalHandle, TextToSpeechRequest, UpdateMediaAssetRequest, Workspace, WorkspaceItem,
    WorkspaceMediaFile, WorkspaceScanEvent, WorkspaceScanSummary,
};
use window::{apply_always_on_top, configure_window, register_hotkey, WindowState};
use workspace::{
    build_context_prompt, create_workspace_text_file as create_workspace_fs_text_file,
    delete_workspace_path as delete_workspace_fs_path,
    rename_workspace_path as rename_workspace_fs_path, scan_workspace,
};

struct AppState {
    db: Database,
    providers: ProviderService,
    streams: Mutex<std::collections::HashMap<String, CancellationToken>>,
    terminals: TerminalRegistry,
    hands: HandsService,
    agent_mgr: agent_manager::AgentManager,
}

#[tauri::command]
async fn save_api_key(
    state: State<'_, AppState>,
    provider: ProviderId,
    api_key: String,
) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("API key cannot be empty.".into());
    }
    state
        .providers
        .save_api_key(provider, api_key.trim())
        .map_err(to_command_error)
}

#[tauri::command]
async fn delete_api_key(state: State<'_, AppState>, provider: ProviderId) -> Result<(), String> {
    state
        .providers
        .delete_api_key(provider)
        .map_err(to_command_error)
}

#[tauri::command]
async fn get_provider_status(state: State<'_, AppState>) -> Result<Vec<ProviderStatus>, String> {
    let mut statuses = Vec::new();
    for provider in [ProviderId::Xai] {
        let configured = state
            .providers
            .has_key(provider)
            .map_err(to_command_error)?;
        statuses.push(ProviderStatus {
            provider_id: provider,
            configured,
            available: configured,
            error: None,
        });
    }
    Ok(statuses)
}

#[tauri::command]
async fn list_models(
    state: State<'_, AppState>,
    provider: Option<ProviderId>,
) -> Result<Vec<ModelDescriptor>, String> {
    state
        .providers
        .list_models(provider)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
fn read_media_data_url(file_path: String) -> Result<String, String> {
    let media_root = app_storage_dir().join("media");
    let canonical_root = media_root.canonicalize().map_err(|error| {
        to_command_error(AppError::message(format!(
            "media root unavailable: {error}"
        )))
    })?;
    let canonical_path = std::path::PathBuf::from(&file_path)
        .canonicalize()
        .map_err(to_command_error)?;

    if !canonical_path.starts_with(&canonical_root) {
        return Err("media file path is outside the app media directory".into());
    }

    let bytes = std::fs::read(&canonical_path).map_err(to_command_error)?;
    let mime = detect_media_mime(&canonical_path, &bytes);
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    state.db.load_settings().map_err(to_command_error)
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SettingsPatch,
) -> Result<Settings, String> {
    let settings = state.db.update_settings(input).map_err(to_command_error)?;
    apply_always_on_top(&app, settings.always_on_top).map_err(to_command_error)?;
    register_hotkey(&app, &settings.hotkey).map_err(to_command_error)?;
    Ok(settings)
}

#[tauri::command]
fn create_conversation(
    state: State<'_, AppState>,
    input: NewConversation,
) -> Result<Conversation, String> {
    state
        .db
        .create_conversation(input)
        .map_err(to_command_error)
}

#[tauri::command]
fn list_conversations(state: State<'_, AppState>) -> Result<Vec<ConversationSummary>, String> {
    state.db.list_conversations().map_err(to_command_error)
}

#[tauri::command]
fn load_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<ConversationDetail, String> {
    state
        .db
        .load_conversation(&conversation_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn rename_conversation(
    state: State<'_, AppState>,
    input: RenameConversation,
) -> Result<(), String> {
    state
        .db
        .rename_conversation(&input.conversation_id, &input.title)
        .map_err(to_command_error)
}

#[tauri::command]
fn set_conversation_pinned(
    state: State<'_, AppState>,
    conversation_id: String,
    pinned: bool,
) -> Result<(), String> {
    state
        .db
        .set_conversation_pinned(&conversation_id, pinned)
        .map_err(to_command_error)
}

#[tauri::command]
fn delete_conversation(state: State<'_, AppState>, conversation_id: String) -> Result<(), String> {
    state
        .db
        .delete_conversation(&conversation_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn create_workspace(state: State<'_, AppState>, input: NewWorkspace) -> Result<Workspace, String> {
    state.db.create_workspace(input).map_err(to_command_error)
}

#[tauri::command]
fn update_workspace(
    state: State<'_, AppState>,
    workspace_id: String,
    input: NewWorkspace,
) -> Result<Workspace, String> {
    state
        .db
        .update_workspace(&workspace_id, input)
        .map_err(to_command_error)
}

#[tauri::command]
fn list_workspaces(state: State<'_, AppState>) -> Result<Vec<Workspace>, String> {
    state.db.list_workspaces().map_err(to_command_error)
}

#[tauri::command]
fn delete_workspace(state: State<'_, AppState>, workspace_id: String) -> Result<(), String> {
    state
        .db
        .delete_workspace(&workspace_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn scan_workspace_command(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<WorkspaceScanSummary, String> {
    app.emit(
        "workspace://scan",
        WorkspaceScanEvent {
            workspace_id: workspace_id.clone(),
            phase: "started".into(),
            scanned_files: 0,
            indexed_items: 0,
            message: None,
        },
    )
    .map_err(to_command_error)?;
    let workspace = state
        .db
        .get_workspace(&workspace_id)
        .map_err(to_command_error)?
        .ok_or_else(|| "Workspace not found.".to_string())?;
    let (summary, items) =
        scan_workspace(&workspace.id, &workspace.roots).map_err(to_command_error)?;
    state
        .db
        .replace_workspace_items(&workspace.id, &items)
        .map_err(to_command_error)?;
    app.emit(
        "workspace://scan",
        WorkspaceScanEvent {
            workspace_id,
            phase: "completed".into(),
            scanned_files: summary.scanned_files,
            indexed_items: summary.indexed_items,
            message: None,
        },
    )
    .map_err(to_command_error)?;
    Ok(summary)
}

#[tauri::command]
fn list_workspace_items(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<WorkspaceItem>, String> {
    state
        .db
        .list_workspace_items(&workspace_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn read_workspace_text_file(
    state: State<'_, AppState>,
    file_path: String,
) -> Result<String, String> {
    validate_workspace_file_path(&state, &file_path)?;
    std::fs::read_to_string(&file_path).map_err(to_command_error)
}

#[tauri::command]
fn write_workspace_text_file(
    state: State<'_, AppState>,
    file_path: String,
    content: String,
) -> Result<(), String> {
    validate_workspace_file_path(&state, &file_path)?;
    std::fs::write(&file_path, &content).map_err(to_command_error)?;
    state
        .db
        .refresh_workspace_item_content_by_path(&file_path, &content)
        .map_err(to_command_error)
}

#[tauri::command]
fn create_workspace_text_file(
    state: State<'_, AppState>,
    file_path: String,
    content: String,
) -> Result<(), String> {
    validate_workspace_file_path(&state, &file_path)?;
    create_workspace_fs_text_file(&file_path, &content).map_err(to_command_error)
}

#[tauri::command]
fn rename_workspace_path(state: State<'_, AppState>, path: String, new_name: String) -> Result<(), String> {
    validate_workspace_file_path(&state, &path)?;
    rename_workspace_fs_path(&path, &new_name).map_err(to_command_error)
}

#[tauri::command]
fn delete_workspace_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    validate_workspace_file_path(&state, &path)?;
    delete_workspace_fs_path(&path).map_err(to_command_error)
}

#[tauri::command]
fn list_workspace_media(
    state: State<'_, AppState>,
    workspace_id: String,
    kind: Option<String>,
) -> Result<Vec<WorkspaceMediaFile>, String> {
    let workspace = state
        .db
        .get_workspace(&workspace_id)
        .map_err(to_command_error)?
        .ok_or_else(|| "Workspace not found.".to_string())?;
    collect_workspace_media(&workspace.roots, kind.as_deref()).map_err(to_command_error)
}

#[tauri::command]
fn create_media_category(
    state: State<'_, AppState>,
    input: NewMediaCategory,
) -> Result<MediaCategory, String> {
    state
        .db
        .create_media_category(input)
        .map_err(to_command_error)
}

#[tauri::command]
fn list_media_categories(state: State<'_, AppState>) -> Result<Vec<MediaCategory>, String> {
    state.db.list_media_categories().map_err(to_command_error)
}

#[tauri::command]
fn list_media_assets(
    state: State<'_, AppState>,
    category_id: Option<String>,
) -> Result<Vec<MediaAsset>, String> {
    state
        .db
        .list_media_assets(category_id.as_deref())
        .map_err(to_command_error)
}

#[tauri::command]
fn import_local_media_command(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ImportLocalMediaRequest,
) -> Result<MediaAsset, String> {
    let asset = import_local_media_asset(&app, input).map_err(to_command_error)?;
    state
        .db
        .insert_media_asset(&asset)
        .map_err(to_command_error)?;
    Ok(asset)
}

#[tauri::command]
fn update_media_asset_category(
    state: State<'_, AppState>,
    asset_id: String,
    input: UpdateMediaAssetRequest,
) -> Result<MediaAsset, String> {
    state
        .db
        .update_media_asset_category(&asset_id, input)
        .map_err(to_command_error)
}

#[tauri::command]
fn delete_media_asset(state: State<'_, AppState>, asset_id: String) -> Result<(), String> {
    state
        .db
        .delete_media_asset(&asset_id)
        .map_err(to_command_error)
}

#[tauri::command]
async fn generate_image_command(
    app: AppHandle,
    state: State<'_, AppState>,
    input: GenerateImageRequest,
) -> Result<MediaAsset, String> {
    let asset = state
        .providers
        .generate_image(&input, &media_output_dir(&app, "images"))
        .await
        .map_err(to_command_error)?;
    state
        .db
        .insert_media_asset(&asset)
        .map_err(to_command_error)?;
    Ok(asset)
}

#[tauri::command]
async fn generate_video_command(
    app: AppHandle,
    state: State<'_, AppState>,
    input: GenerateVideoRequest,
) -> Result<MediaAsset, String> {
    let asset = state
        .providers
        .generate_video(&input, &media_output_dir(&app, "videos"))
        .await
        .map_err(to_command_error)?;
    state
        .db
        .insert_media_asset(&asset)
        .map_err(to_command_error)?;
    Ok(asset)
}

#[tauri::command]
async fn text_to_speech_command(
    app: AppHandle,
    state: State<'_, AppState>,
    input: TextToSpeechRequest,
) -> Result<MediaAsset, String> {
    let asset = state
        .providers
        .text_to_speech(&input, &media_output_dir(&app, "audio"))
        .await
        .map_err(to_command_error)?;
    state
        .db
        .insert_media_asset(&asset)
        .map_err(to_command_error)?;
    Ok(asset)
}

#[tauri::command]
async fn export_editor_timeline_command(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ExportEditorTimelineRequest,
) -> Result<MediaAsset, String> {
    let asset = editor::export_timeline(&input, &media_output_dir(&app, "exports"))
        .await
        .map_err(to_command_error)?;
    state
        .db
        .insert_media_asset(&asset)
        .map_err(to_command_error)?;
    Ok(asset)
}

#[tauri::command]
async fn create_realtime_session_command(
    state: State<'_, AppState>,
    input: RealtimeSessionRequest,
) -> Result<RealtimeSession, String> {
    state
        .providers
        .create_realtime_session(&input)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ChatRequest,
) -> Result<StreamHandle, String> {
    let trimmed = input.user_text.trim();
    if trimmed.is_empty() {
        return Err("Message cannot be empty.".into());
    }

    let selected_items = state
        .db
        .fetch_workspace_items_by_ids(&input.selected_workspace_items)
        .map_err(to_command_error)?;
    let workspace_context = build_context_prompt(&selected_items).map_err(to_command_error)?;

    let user_message = state
        .db
        .insert_message(
            &input.conversation_id,
            types::MessageRole::User,
            trimmed,
            "complete",
            Some(input.provider_id),
            Some(&input.model_id),
        )
        .map_err(to_command_error)?;
    state
        .db
        .save_message_context(&user_message.id, &input.selected_workspace_items)
        .map_err(to_command_error)?;
    let assistant_message = state
        .db
        .insert_message(
            &input.conversation_id,
            types::MessageRole::Assistant,
            "",
            "streaming",
            Some(input.provider_id),
            Some(&input.model_id),
        )
        .map_err(to_command_error)?;
    let history = state
        .db
        .build_chat_history(&input)
        .map_err(to_command_error)?;

    let stream_id = uuid::Uuid::new_v4().to_string();
    let handle = StreamHandle {
        stream_id: stream_id.clone(),
        message_id: assistant_message.id.clone(),
    };
    let cancel = CancellationToken::new();
    state
        .streams
        .lock()
        .map_err(|_| "stream registry lock poisoned".to_string())?
        .insert(stream_id.clone(), cancel.clone());

    let db = state.db.clone();
    let providers = state.providers.clone();
    tauri::async_runtime::spawn(async move {
        let mut aggregate = String::new();
        let mut part_index = 0usize;
        let emit_started = app.emit(
            "chat://stream",
            StreamEvent {
                stream_id: stream_id.clone(),
                kind: "started".into(),
                text_delta: None,
                message_id: assistant_message.id.clone(),
                usage: None,
                error: None,
            },
        );
        if let Err(error) = emit_started {
            error!("failed to emit stream start: {error}");
        }

        let result = providers
            .stream_chat(
                input.provider_id,
                &input.model_id,
                &history,
                &workspace_context,
                input.temperature,
                input.max_output_tokens,
                cancel.clone(),
                |delta| {
                    aggregate.push_str(&delta);
                    db.append_message_part(&assistant_message.id, part_index, &delta)?;
                    part_index += 1;
                    app.emit(
                        "chat://stream",
                        StreamEvent {
                            stream_id: stream_id.clone(),
                            kind: "delta".into(),
                            text_delta: Some(delta),
                            message_id: assistant_message.id.clone(),
                            usage: None,
                            error: None,
                        },
                    )?;
                    Ok(())
                },
            )
            .await;

        match result {
            Ok(usage) => {
                if let Err(error) = db.finalize_message(
                    &assistant_message.id,
                    &aggregate,
                    "complete",
                    Some(usage.clone()),
                    None,
                ) {
                    error!("failed to finalize assistant message: {error}");
                }
                let _ = app.emit(
                    "chat://stream",
                    StreamEvent {
                        stream_id: stream_id.clone(),
                        kind: "completed".into(),
                        text_delta: None,
                        message_id: assistant_message.id.clone(),
                        usage: Some(usage),
                        error: None,
                    },
                );
            }
            Err(error) if error.to_string() == "cancelled" => {
                if let Err(db_error) =
                    db.finalize_message(&assistant_message.id, &aggregate, "cancelled", None, None)
                {
                    error!("failed to store cancelled message: {db_error}");
                }
                let _ = app.emit(
                    "chat://stream",
                    StreamEvent {
                        stream_id: stream_id.clone(),
                        kind: "cancelled".into(),
                        text_delta: None,
                        message_id: assistant_message.id.clone(),
                        usage: None,
                        error: None,
                    },
                );
            }
            Err(error) => {
                let message = error.to_string();
                if let Err(db_error) = db.finalize_message(
                    &assistant_message.id,
                    &aggregate,
                    "error",
                    None,
                    Some(message.clone()),
                ) {
                    error!("failed to store errored message: {db_error}");
                }
                let _ = app.emit(
                    "chat://stream",
                    StreamEvent {
                        stream_id: stream_id.clone(),
                        kind: "error".into(),
                        text_delta: None,
                        message_id: assistant_message.id.clone(),
                        usage: None,
                        error: Some(message.clone()),
                    },
                );
            }
        }

        if let Ok(mut registry) = app
            .state::<AppState>()
            .streams
            .lock()
            .map_err(|_| AppError::message("stream registry lock poisoned"))
        {
            registry.remove(&stream_id);
        }
    });

    Ok(handle)
}

#[tauri::command]
fn cancel_stream(state: State<'_, AppState>, stream_id: String) -> Result<(), String> {
    let registry = state
        .streams
        .lock()
        .map_err(|_| "stream registry lock poisoned".to_string())?;
    if let Some(token) = registry.get(&stream_id) {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn list_active_agents(
    state: State<'_, AppState>,
) -> Result<Vec<agent_manager::AgentInfo>, String> {
    Ok(state.agent_mgr.list_agents().await)
}

#[tauri::command]
async fn cancel_agent(
    state: State<'_, AppState>,
    agent_id: String,
) -> Result<(), String> {
    state.agent_mgr.cancel_agent(&agent_id).await;
    Ok(())
}

#[tauri::command]
async fn send_agent_message(
    app: AppHandle,
    state: State<'_, AppState>,
    input: AgentChatRequest,
) -> Result<StreamHandle, String> {
    let trimmed = input.user_text.trim();
    if trimmed.is_empty() {
        return Err("Message cannot be empty.".into());
    }

    let selected_items = state
        .db
        .fetch_workspace_items_by_ids(&input.selected_workspace_items)
        .map_err(to_command_error)?;
    let workspace_context = build_context_prompt(&selected_items).map_err(to_command_error)?;

    let user_message = state
        .db
        .insert_message(
            &input.conversation_id,
            types::MessageRole::User,
            trimmed,
            "complete",
            Some(input.provider_id),
            Some(&input.model_id),
        )
        .map_err(to_command_error)?;
    state
        .db
        .save_message_context(&user_message.id, &input.selected_workspace_items)
        .map_err(to_command_error)?;
    let assistant_message = state
        .db
        .insert_message(
            &input.conversation_id,
            types::MessageRole::Assistant,
            "",
            "streaming",
            Some(input.provider_id),
            Some(&input.model_id),
        )
        .map_err(to_command_error)?;
    let history = state
        .db
        .build_chat_history(&ChatRequest {
            conversation_id: input.conversation_id.clone(),
            provider_id: input.provider_id,
            model_id: input.model_id.clone(),
            user_text: trimmed.to_string(),
            selected_workspace_items: input.selected_workspace_items.clone(),
            temperature: input.temperature,
            max_output_tokens: input.max_output_tokens,
        })
        .map_err(to_command_error)?;

    let stream_id = uuid::Uuid::new_v4().to_string();
    let handle = StreamHandle {
        stream_id: stream_id.clone(),
        message_id: assistant_message.id.clone(),
    };
    let cancel = CancellationToken::new();
    state
        .streams
        .lock()
        .map_err(|_| "stream registry lock poisoned".to_string())?
        .insert(stream_id.clone(), cancel.clone());

    // Collect workspace roots for the agent
    let workspaces = state.db.list_workspaces().map_err(to_command_error)?;
    let workspace_roots: Vec<String> = workspaces
        .iter()
        .flat_map(|w| w.roots.clone())
        .collect();

    let db = state.db.clone();
    let providers = state.providers.clone();
    let api_key = providers.require_api_key_public().map_err(to_command_error)?;

    tauri::async_runtime::spawn(async move {
        let emit_started = app.emit(
            "chat://stream",
            StreamEvent {
                stream_id: stream_id.clone(),
                kind: "started".into(),
                text_delta: None,
                message_id: assistant_message.id.clone(),
                usage: None,
                error: None,
            },
        );
        if let Err(e) = emit_started {
            error!("failed to emit stream start: {e}");
        }

        // Emit agent events through a separate event channel
        let app_ref = app.clone();
        let stream_id_ref = stream_id.clone();
        let msg_id_ref = assistant_message.id.clone();

        // Build OpenAI-format history
        let mut openai_messages: Vec<serde_json::Value> = Vec::new();
        for msg in &history {
            if msg.role == "system" {
                continue;
            }
            openai_messages.push(serde_json::json!({
                "role": if msg.role == "assistant" { "assistant" } else { "user" },
                "content": msg.content,
            }));
        }

        let system_prompt = providers::base_system_prompt(&workspace_context);

        let agent_config = agent::AgentConfig {
            model_id: input.model_id.clone(),
            system_prompt,
            max_iterations: input.max_iterations.unwrap_or(25) as usize,
            workspace_roots: workspace_roots.clone(),
        };
        let tool_registry = tools::ToolRegistry::new(workspace_roots);

        let result = agent::run_agent(
            providers.client(),
            &api_key,
            &agent_config,
            openai_messages,
            &tool_registry,
            cancel.clone(),
            |event| {
                // Forward agent events to frontend
                let _ = app_ref.emit("agent://event", serde_json::json!({
                    "streamId": &stream_id_ref,
                    "messageId": &msg_id_ref,
                    "event": event,
                }));
                // Also emit text deltas through the regular stream channel
                if let agent::AgentEvent::TextDelta { ref text } = event {
                    let _ = app_ref.emit(
                        "chat://stream",
                        StreamEvent {
                            stream_id: stream_id_ref.clone(),
                            kind: "delta".into(),
                            text_delta: Some(text.clone()),
                            message_id: msg_id_ref.clone(),
                            usage: None,
                            error: None,
                        },
                    );
                }
                Ok(())
            },
        )
        .await;

        match result {
            Ok(agent_result) => {
                let usage = agent_result.usage;
                if let Err(e) = db.finalize_message(
                    &assistant_message.id,
                    &agent_result.final_text,
                    "complete",
                    Some(usage.clone()),
                    None,
                ) {
                    error!("failed to finalize agent message: {e}");
                }
                // Store tool call records as metadata
                if !agent_result.tool_calls_made.is_empty() {
                    if let Ok(json) = serde_json::to_string(&agent_result.tool_calls_made) {
                        let _ = db.save_message_tool_calls(&assistant_message.id, &json);
                    }
                }
                let _ = app.emit(
                    "chat://stream",
                    StreamEvent {
                        stream_id: stream_id.clone(),
                        kind: "completed".into(),
                        text_delta: None,
                        message_id: assistant_message.id.clone(),
                        usage: Some(usage),
                        error: None,
                    },
                );
            }
            Err(e) if e.to_string() == "cancelled" => {
                let _ = db.finalize_message(
                    &assistant_message.id,
                    "",
                    "cancelled",
                    None,
                    None,
                );
                let _ = app.emit(
                    "chat://stream",
                    StreamEvent {
                        stream_id: stream_id.clone(),
                        kind: "cancelled".into(),
                        text_delta: None,
                        message_id: assistant_message.id.clone(),
                        usage: None,
                        error: None,
                    },
                );
            }
            Err(e) => {
                let message = e.to_string();
                let _ = db.finalize_message(
                    &assistant_message.id,
                    "",
                    "error",
                    None,
                    Some(message.clone()),
                );
                let _ = app.emit(
                    "chat://stream",
                    StreamEvent {
                        stream_id: stream_id.clone(),
                        kind: "error".into(),
                        text_delta: None,
                        message_id: assistant_message.id.clone(),
                        usage: None,
                        error: Some(message),
                    },
                );
            }
        }

        if let Ok(mut registry) = app
            .state::<AppState>()
            .streams
            .lock()
            .map_err(|_| AppError::message("stream registry lock poisoned"))
        {
            registry.remove(&stream_id);
        }
    });

    Ok(handle)
}

#[tauri::command]
async fn start_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TerminalHandle, String> {
    ensure_terminal(app, &state.terminals)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn write_terminal_input(
    state: State<'_, AppState>,
    session_id: String,
    input: String,
) -> Result<(), String> {
    write_input(&state.terminals, &session_id, &input)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn kill_terminal(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    terminate_terminal(&state.terminals, &session_id)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn resize_terminal_command(
    state: State<'_, AppState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    resize_terminal(&state.terminals, &session_id, cols, rows)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn get_hands_status(state: State<'_, AppState>) -> Result<HandsStatus, String> {
    let settings = state.db.load_settings().map_err(to_command_error)?;
    Ok(state.hands.snapshot(&settings).await)
}

#[tauri::command]
async fn start_hands_service(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<HandsStatus, String> {
    let settings = state.db.load_settings().map_err(to_command_error)?;
    state
        .hands
        .start(app, settings)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn stop_hands_service(state: State<'_, AppState>) -> Result<HandsStatus, String> {
    let settings = state.db.load_settings().map_err(to_command_error)?;
    Ok(state.hands.stop(&settings).await)
}

fn validate_workspace_file_path(state: &AppState, file_path: &str) -> Result<(), String> {
    let workspaces = state.db.list_workspaces().map_err(to_command_error)?;
    let target = std::path::PathBuf::from(file_path);
    // Allow both existing and not-yet-existing paths by canonicalizing the parent
    let canonical = if target.exists() {
        target
            .canonicalize()
            .map_err(|e| format!("path resolution failed: {e}"))?
    } else {
        let parent = target
            .parent()
            .ok_or_else(|| "invalid file path".to_string())?;
        if parent.exists() {
            let mut resolved = parent
                .canonicalize()
                .map_err(|e| format!("path resolution failed: {e}"))?;
            if let Some(name) = target.file_name() {
                resolved.push(name);
            }
            resolved
        } else {
            target.clone()
        }
    };
    for workspace in &workspaces {
        for root in &workspace.roots {
            let root_path = std::path::PathBuf::from(root);
            let canonical_root = if root_path.exists() {
                root_path.canonicalize().unwrap_or_else(|_| root_path.clone())
            } else {
                root_path
            };
            if canonical.starts_with(&canonical_root) {
                return Ok(());
            }
        }
    }
    Err("file path is outside all workspace roots".into())
}

fn to_command_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn app_storage_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("GrokDesktop")
}

fn media_output_dir(_app: &AppHandle, bucket: &str) -> std::path::PathBuf {
    app_storage_dir().join("media").join(bucket)
}

fn collect_workspace_media(
    roots: &[String],
    kind_filter: Option<&str>,
) -> Result<Vec<WorkspaceMediaFile>, AppError> {
    let mut items = Vec::new();

    for root in roots {
        let root_path = std::path::PathBuf::from(root);
        if root_path.is_file() {
            if let Some(item) = workspace_media_entry(&root_path, kind_filter)? {
                items.push(item);
            }
            continue;
        }

        for entry in walkdir::WalkDir::new(&root_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry
                    .path()
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(|name| {
                        !matches!(name, ".git" | "node_modules" | "target" | ".next" | "dist")
                    })
                    .unwrap_or(true)
            })
        {
            let entry = match entry {
                Ok(value) => value,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(item) = workspace_media_entry(path, kind_filter)? {
                items.push(item);
            }
        }
    }

    items.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    Ok(items)
}

fn workspace_media_entry(
    path: &std::path::Path,
    kind_filter: Option<&str>,
) -> Result<Option<WorkspaceMediaFile>, AppError> {
    let mime = mime_guess::from_path(path)
        .first_raw()
        .map(ToString::to_string)
        .or_else(|| Some(detect_media_mime(path, &[])));
    let Some(kind) = mime.as_deref().and_then(classify_media_kind) else {
        return Ok(None);
    };
    if let Some(filter) = kind_filter {
        if filter != kind {
            return Ok(None);
        }
    }
    let metadata = std::fs::metadata(path)?;
    Ok(Some(WorkspaceMediaFile {
        path: path.to_string_lossy().to_string(),
        kind: kind.to_string(),
        mime_type: mime,
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string(),
        file_size: metadata.len(),
    }))
}

fn import_local_media_asset(
    app: &AppHandle,
    input: ImportLocalMediaRequest,
) -> Result<MediaAsset, AppError> {
    let source = std::path::PathBuf::from(&input.file_path);
    if !source.exists() || !source.is_file() {
        return Err(AppError::message("Local media file is missing."));
    }

    let bytes = std::fs::read(&source)?;
    let mime = detect_media_mime(&source, &bytes);
    let kind = classify_media_kind(&mime)
        .ok_or_else(|| AppError::message("Only image, video, and audio files can be imported."))?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(match kind {
            "image" => "png",
            "video" => "mp4",
            "audio" => "mp3",
            _ => "bin",
        });
    let bucket = match kind {
        "image" => "imports/images",
        "video" => "imports/videos",
        "audio" => "imports/audio",
        _ => "imports",
    };
    let output_dir = media_output_dir(app, bucket);
    std::fs::create_dir_all(&output_dir)?;
    let output_path = output_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
    std::fs::write(&output_path, bytes)?;

    let now = chrono::Utc::now().to_rfc3339();
    Ok(MediaAsset {
        id: uuid::Uuid::new_v4().to_string(),
        category_id: input.category_id,
        kind: kind.to_string(),
        model_id: "local-import".into(),
        prompt: input
            .prompt
            .and_then(|value| {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            })
            .unwrap_or_else(|| {
                source
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("Local media")
                    .to_string()
            }),
        file_path: output_path.to_string_lossy().to_string(),
        source_url: Some(source.to_string_lossy().to_string()),
        mime_type: Some(mime),
        status: "completed".into(),
        request_id: None,
        metadata_json: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

fn classify_media_kind(mime: &str) -> Option<&'static str> {
    if mime.starts_with("image/") {
        return Some("image");
    }
    if mime.starts_with("video/") {
        return Some("video");
    }
    if mime.starts_with("audio/") {
        return Some("audio");
    }
    None
}

fn detect_media_mime(path: &std::path::Path, bytes: &[u8]) -> String {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return "image/jpeg".into();
    }
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
    {
        return "image/png".into();
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp".into();
    }
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return "video/mp4".into();
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return "audio/wav".into();
    }
    if bytes.len() >= 3 && &bytes[0..3] == b"ID3" {
        return "audio/mpeg".into();
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        return "audio/mpeg".into();
    }
    match mime_guess::from_path(path).first_raw() {
        Some(value) => value.to_string(),
        None => "application/octet-stream".into(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,grok_desktop_lib=info".into()),
        )
        .init();

    let app_data_dir = app_storage_dir();
    let database = Database::new(app_data_dir.join("grokdesktop.sqlite"));
    database.init().expect("database initialization failed");

    let secrets: Arc<dyn SecretStore> = Arc::new(MigratingSecretStore::new(
        app_data_dir.join("secrets"),
        "com.megabrain2.grokdesktop",
    ));
    let providers = ProviderService::new(reqwest::Client::new(), secrets);
    let initial_settings = database.load_settings().expect("settings load failed");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(WindowState::default())
        .manage(AppState {
            db: database.clone(),
            providers: providers.clone(),
            streams: Mutex::new(std::collections::HashMap::new()),
            terminals: Mutex::new(std::collections::HashMap::new()),
            hands: HandsService::new(database.clone(), providers),
            agent_mgr: agent_manager::AgentManager::new(5, 100),
        })
        .setup(move |app| {
            configure_window(app)?;
            apply_always_on_top(&app.handle(), initial_settings.always_on_top)?;
            register_hotkey(&app.handle(), &initial_settings.hotkey)?;
            info!("Grok Desktop ready");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            save_api_key,
            delete_api_key,
            get_provider_status,
            list_models,
            read_media_data_url,
            get_settings,
            update_settings,
            create_conversation,
            list_conversations,
            load_conversation,
            rename_conversation,
            set_conversation_pinned,
            delete_conversation,
            send_message,
            send_agent_message,
            list_active_agents,
            cancel_agent,
            cancel_stream,
            start_terminal,
            write_terminal_input,
            kill_terminal,
            resize_terminal_command,
            get_hands_status,
            start_hands_service,
            stop_hands_service,
            create_workspace,
            update_workspace,
            list_workspaces,
            delete_workspace,
            scan_workspace_command,
            list_workspace_items,
            read_workspace_text_file,
            write_workspace_text_file,
            create_workspace_text_file,
            rename_workspace_path,
            delete_workspace_path,
            list_workspace_media,
            create_media_category,
            list_media_categories,
            list_media_assets,
            import_local_media_command,
            update_media_asset_category,
            delete_media_asset,
            generate_image_command,
            generate_video_command,
            export_editor_timeline_command,
            text_to_speech_command,
            create_realtime_session_command
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
