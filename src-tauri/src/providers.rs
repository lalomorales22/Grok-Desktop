use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::keychain::SecretStore;
use crate::types::Message;
use crate::types::{
    GenerateImageRequest, GenerateVideoRequest, MediaAsset, ModelDescriptor, ProviderId,
    RealtimeSession, RealtimeSessionRequest, TaskType, TextToSpeechRequest, TokenUsage,
};

const XAI_CHAT_ENDPOINT: &str = "https://api.x.ai/v1/chat/completions";
const XAI_IMAGE_ENDPOINT: &str = "https://api.x.ai/v1/images/generations";
const XAI_VIDEO_ENDPOINT: &str = "https://api.x.ai/v1/videos/generations";
const XAI_VIDEO_STATUS_ENDPOINT: &str = "https://api.x.ai/v1/videos";
const XAI_REALTIME_SECRET_ENDPOINT: &str = "https://api.x.ai/v1/realtime/client_secrets";
const XAI_REALTIME_WEBSOCKET_URL: &str = "wss://api.x.ai/v1/realtime";
const XAI_TTS_ENDPOINT: &str = "https://api.x.ai/v1/tts";

#[derive(Clone)]
pub struct ProviderService {
    client: Client,
    secrets: Arc<dyn SecretStore>,
}

impl ProviderService {
    pub fn new(client: Client, secrets: Arc<dyn SecretStore>) -> Self {
        Self { client, secrets }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn require_api_key_public(&self) -> AppResult<String> {
        self.require_api_key()
    }

    pub fn has_key(&self, provider: ProviderId) -> AppResult<bool> {
        self.secrets.has_api_key(provider)
    }

    pub fn save_api_key(&self, provider: ProviderId, api_key: &str) -> AppResult<()> {
        self.secrets.set_api_key(provider, api_key)
    }

    pub fn delete_api_key(&self, provider: ProviderId) -> AppResult<()> {
        self.secrets.delete_api_key(provider)
    }

    pub async fn list_models(
        &self,
        _provider: Option<ProviderId>,
    ) -> AppResult<Vec<ModelDescriptor>> {
        Ok(chat_models())
    }

    pub async fn stream_chat<F>(
        &self,
        _provider: ProviderId,
        model_id: &str,
        history: &[Message],
        workspace_context: &str,
        _temperature: Option<f32>,
        max_output_tokens: Option<u32>,
        cancel: CancellationToken,
        mut on_delta: F,
    ) -> AppResult<TokenUsage>
    where
        F: FnMut(String) -> AppResult<()>,
    {
        let api_key = self.require_api_key()?;

        let mut messages = vec![serde_json::json!({
            "role": "system",
            "content": base_system_prompt(workspace_context),
        })];
        messages.extend(
            history
                .iter()
                .filter(|message| message.role != "system")
                .map(|message| {
                    serde_json::json!({
                        "role": if message.role == "assistant" { "assistant" } else { "user" },
                        "content": message.content,
                    })
                }),
        );

        // Prune context if approaching the model's context window limit.
        // Keep the 10 most recent messages to preserve conversational coherence.
        prune_context(&mut messages, model_id, 10);

        let effective_max_tokens = max_output_tokens
            .unwrap_or_else(|| model_default_max_output(model_id));
        let request_body = serde_json::json!({
            "model": model_id,
            "stream": true,
            "messages": messages,
            "max_tokens": effective_max_tokens,
            "stream_options": { "include_usage": true },
        });

        let response = self
            .client
            .post(XAI_CHAT_ENDPOINT)
            .bearer_auth(api_key)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::message(extract_error(response).await?));
        }

        let mut usage = TokenUsage {
            input_tokens: None,
            output_tokens: None,
        };
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            if cancel.is_cancelled() {
                return Err(AppError::message("cancelled"));
            }
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(event) = take_sse_event(&mut buffer) {
                let data = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join("");
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let json: Value = serde_json::from_str(&data)?;
                if let Some(delta) = json
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|choices| choices.first())
                    .and_then(|choice| choice.get("delta"))
                    .and_then(|delta| delta.get("content"))
                    .and_then(Value::as_str)
                {
                    on_delta(delta.to_string())?;
                }
                if let Some(usage_value) = json.get("usage") {
                    usage.input_tokens = usage_value
                        .get("prompt_tokens")
                        .and_then(Value::as_u64)
                        .or_else(|| usage_value.get("input_tokens").and_then(Value::as_u64));
                    usage.output_tokens = usage_value
                        .get("completion_tokens")
                        .and_then(Value::as_u64)
                        .or_else(|| usage_value.get("output_tokens").and_then(Value::as_u64));
                }
                if let Some(message) = json
                    .get("error")
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                {
                    return Err(AppError::message(message));
                }
            }
        }
        Ok(usage)
    }

    pub async fn generate_image(
        &self,
        request: &GenerateImageRequest,
        output_dir: &Path,
    ) -> AppResult<MediaAsset> {
        let api_key = self.require_api_key()?;
        std::fs::create_dir_all(output_dir)?;

        let response = self
            .client
            .post(XAI_IMAGE_ENDPOINT)
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "model": request.model_id,
                "prompt": request.prompt,
                "aspect_ratio": request.aspect_ratio.clone().unwrap_or_else(|| "1:1".to_string()),
                "resolution": request.resolution.clone().unwrap_or_else(|| "1k".to_string()),
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::message(extract_error(response).await?));
        }

        let json: Value = response.json().await?;
        let item = json
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .ok_or_else(|| AppError::message("xAI image generation returned no image data"))?;

        let file_name = format!("{}.png", uuid::Uuid::new_v4());
        let file_path = output_dir.join(file_name);
        let source_url = if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|error| AppError::message(format!("invalid image payload: {error}")))?;
            std::fs::write(&file_path, bytes)?;
            None
        } else if let Some(url) = item.get("url").and_then(Value::as_str) {
            let bytes = self.download_bytes(url).await?;
            std::fs::write(&file_path, bytes)?;
            Some(url.to_string())
        } else {
            return Err(AppError::message(
                "xAI image generation returned no supported image payload",
            ));
        };

        let now = chrono::Utc::now().to_rfc3339();
        Ok(MediaAsset {
            id: uuid::Uuid::new_v4().to_string(),
            category_id: request.category_id.clone(),
            kind: "image".into(),
            model_id: request.model_id.clone(),
            prompt: request.prompt.clone(),
            file_path: file_path.to_string_lossy().to_string(),
            source_url,
            mime_type: Some("image/png".into()),
            status: "completed".into(),
            request_id: None,
            metadata_json: Some(json.to_string()),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub async fn generate_video(
        &self,
        request: &GenerateVideoRequest,
        output_dir: &Path,
    ) -> AppResult<MediaAsset> {
        let api_key = self.require_api_key()?;
        std::fs::create_dir_all(output_dir)?;

        let response = self
            .client
            .post(XAI_VIDEO_ENDPOINT)
            .bearer_auth(&api_key)
            .json(&serde_json::json!({
                "model": request.model_id,
                "prompt": request.prompt,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::message(extract_error(response).await?));
        }

        let initial: Value = response.json().await?;
        let request_id = initial
            .get("request_id")
            .or_else(|| initial.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::message("xAI video generation did not return a request id"))?
            .to_string();

        let final_state = self.poll_video_until_ready(&api_key, &request_id).await?;
        let video_url = find_video_url(&final_state).ok_or_else(|| {
            AppError::message("xAI video generation completed without a video URL")
        })?;
        let file_name = format!("{}.mp4", uuid::Uuid::new_v4());
        let file_path = output_dir.join(file_name);
        let bytes = self.download_bytes(&video_url).await?;
        std::fs::write(&file_path, bytes)?;

        let now = chrono::Utc::now().to_rfc3339();
        Ok(MediaAsset {
            id: uuid::Uuid::new_v4().to_string(),
            category_id: request.category_id.clone(),
            kind: "video".into(),
            model_id: request.model_id.clone(),
            prompt: request.prompt.clone(),
            file_path: file_path.to_string_lossy().to_string(),
            source_url: Some(video_url),
            mime_type: Some("video/mp4".into()),
            status: "completed".into(),
            request_id: Some(request_id),
            metadata_json: Some(final_state.to_string()),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub async fn text_to_speech(
        &self,
        request: &TextToSpeechRequest,
        output_dir: &Path,
    ) -> AppResult<MediaAsset> {
        let api_key = self.require_api_key()?;
        std::fs::create_dir_all(output_dir)?;

        let format = request
            .response_format
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "mp3".to_string());
        let voice_id = request.voice.clone().unwrap_or_else(|| "eve".to_string());
        let output_format = if format == "wav" {
            serde_json::json!({
                "codec": "wav",
                "sample_rate": 24000,
            })
        } else {
            serde_json::json!({
                "codec": "mp3",
                "sample_rate": 24000,
                "bit_rate": 128000,
            })
        };
        let response = self
            .client
            .post(XAI_TTS_ENDPOINT)
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "text": request.input,
                "voice_id": voice_id,
                "output_format": output_format,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::message(extract_error(response).await?));
        }

        let bytes = response.bytes().await?;
        let extension = if format == "wav" { "wav" } else { "mp3" };
        let file_path = output_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
        std::fs::write(&file_path, bytes)?;

        let now = chrono::Utc::now().to_rfc3339();
        Ok(MediaAsset {
            id: uuid::Uuid::new_v4().to_string(),
            category_id: request.category_id.clone(),
            kind: "audio".into(),
            model_id: request
                .model_id
                .clone()
                .unwrap_or_else(|| "xai-tts".to_string()),
            prompt: request.input.clone(),
            file_path: file_path.to_string_lossy().to_string(),
            source_url: None,
            mime_type: Some(if extension == "wav" {
                "audio/wav".into()
            } else {
                "audio/mpeg".into()
            }),
            status: "completed".into(),
            request_id: None,
            metadata_json: None,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub async fn create_realtime_session(
        &self,
        request: &RealtimeSessionRequest,
    ) -> AppResult<RealtimeSession> {
        let api_key = self.require_api_key()?;
        let response = self
            .client
            .post(XAI_REALTIME_SECRET_ENDPOINT)
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "expires_after": { "anchor": "created_at", "seconds": 900 },
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::message(extract_error(response).await?));
        }

        let json: Value = response.json().await?;
        let client_secret = json
            .get("client_secret")
            .and_then(|value| value.get("value").or_else(|| value.get("secret")))
            .or_else(|| json.get("value"))
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::message("xAI realtime client secret was missing"))?
            .to_string();
        let expires_at = json
            .get("client_secret")
            .and_then(|value| value.get("expires_at"))
            .or_else(|| json.get("expires_at"))
            .and_then(Value::as_str)
            .map(ToString::to_string);

        Ok(RealtimeSession {
            client_secret,
            expires_at,
            websocket_url: XAI_REALTIME_WEBSOCKET_URL.into(),
            model_id: request.model_id.clone(),
            voice: request.voice.clone(),
        })
    }

    fn require_api_key(&self) -> AppResult<String> {
        self.secrets
            .get_api_key(ProviderId::Xai)?
            .ok_or_else(|| AppError::message("xAI API key is not configured."))
    }

    async fn poll_video_until_ready(&self, api_key: &str, request_id: &str) -> AppResult<Value> {
        for _ in 0..120 {
            let response = self
                .client
                .get(format!("{XAI_VIDEO_STATUS_ENDPOINT}/{request_id}"))
                .bearer_auth(api_key)
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(AppError::message(extract_error(response).await?));
            }
            let json: Value = response.json().await?;
            let status = json
                .get("status")
                .or_else(|| json.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            match status {
                "completed" | "succeeded" | "success" | "done" => return Ok(json),
                "failed" | "error" => {
                    let message = json
                        .get("error")
                        .and_then(|value| value.get("message").or(Some(value)))
                        .and_then(Value::as_str)
                        .unwrap_or("xAI video generation failed");
                    return Err(AppError::message(message));
                }
                _ => tokio::time::sleep(Duration::from_secs(3)).await,
            }
        }
        Err(AppError::message(
            "Timed out waiting for xAI video generation",
        ))
    }

    async fn download_bytes(&self, url: &str) -> AppResult<Vec<u8>> {
        let response = self.client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(AppError::message(extract_error(response).await?));
        }
        Ok(response.bytes().await?.to_vec())
    }
}

fn chat_models() -> Vec<ModelDescriptor> {
    vec![
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-4-1-fast-reasoning".into(),
            label: "Grok 4.1 Fast Reasoning".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 16_384,
            capabilities: vec![TaskType::Planning, TaskType::Reasoning],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-4-1-fast-non-reasoning".into(),
            label: "Grok 4.1 Fast".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 16_384,
            capabilities: vec![TaskType::QuickAnswer],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-code-fast-1".into(),
            label: "Grok Code Fast".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 32_768,
            capabilities: vec![TaskType::CodeGeneration],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-4-fast-reasoning".into(),
            label: "Grok 4 Fast Reasoning".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 16_384,
            capabilities: vec![TaskType::Planning, TaskType::Reasoning],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-4-fast-non-reasoning".into(),
            label: "Grok 4 Fast".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 16_384,
            capabilities: vec![TaskType::QuickAnswer],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-4-0709".into(),
            label: "Grok 4 (0709)".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 32_768,
            capabilities: vec![TaskType::Reasoning, TaskType::CodeGeneration],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-3-mini".into(),
            label: "Grok 3 Mini".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 8_192,
            capabilities: vec![TaskType::QuickAnswer],
        },
        ModelDescriptor {
            provider_id: ProviderId::Xai,
            model_id: "grok-3".into(),
            label: "Grok 3".into(),
            supports_streaming: true,
            supports_workspace_context: true,
            context_window: 131_072,
            default_max_output: 16_384,
            capabilities: vec![TaskType::Reasoning, TaskType::CodeGeneration, TaskType::Planning],
        },
    ]
}

/// Select the best model for a given task type. Returns the model_id of the
/// first model whose capabilities include the requested task type.
pub fn route_model(task_type: TaskType) -> String {
    let models = chat_models();
    models
        .iter()
        .find(|m| m.capabilities.contains(&task_type))
        .map(|m| m.model_id.clone())
        .unwrap_or_else(|| "grok-4-1-fast-reasoning".into())
}

/// Look up a model's context window size.
pub fn model_context_window(model_id: &str) -> u64 {
    let models = chat_models();
    models
        .iter()
        .find(|m| m.model_id == model_id)
        .map(|m| m.context_window)
        .unwrap_or(131_072)
}

/// Look up a model's default max output tokens.
pub fn model_default_max_output(model_id: &str) -> u32 {
    let models = chat_models();
    models
        .iter()
        .find(|m| m.model_id == model_id)
        .map(|m| m.default_max_output)
        .unwrap_or(16_384)
}

// ---------------------------------------------------------------------------
// Context window management
// ---------------------------------------------------------------------------

/// Rough estimate of tokens in a string (approx 4 chars per token for English).
fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64 + 3) / 4
}

/// Estimate token count for a slice of messages in OpenAI JSON format.
pub fn estimate_message_tokens(messages: &[Value]) -> u64 {
    let mut total: u64 = 0;
    for msg in messages {
        // Each message has ~4 tokens of overhead (role, delimiters)
        total += 4;
        if let Some(content) = msg.get("content").and_then(Value::as_str) {
            total += estimate_tokens(content);
        }
        // Tool calls add overhead too
        if let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                total += 4; // call overhead
                if let Some(args) = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                {
                    total += estimate_tokens(args);
                }
            }
        }
    }
    total
}

/// When the conversation history approaches the context window limit, prune
/// older messages while preserving the system prompt (first message) and the
/// most recent `keep_recent` messages. Middle messages are replaced with a
/// single summary message.
pub fn prune_context(
    messages: &mut Vec<Value>,
    model_id: &str,
    keep_recent: usize,
) {
    let window = model_context_window(model_id);
    // Leave room for output tokens + some buffer
    let max_input = window.saturating_sub(model_default_max_output(model_id) as u64 + 1024);

    let current_tokens = estimate_message_tokens(messages);
    if current_tokens <= max_input {
        return; // fits fine
    }

    // Need to prune. Keep system prompt (index 0) and the last `keep_recent` messages.
    if messages.len() <= keep_recent + 1 {
        return; // nothing to prune
    }

    let prune_end = messages.len().saturating_sub(keep_recent);
    if prune_end <= 1 {
        return;
    }

    // Summarize the pruned section
    let mut summary_parts: Vec<String> = Vec::new();
    for msg in &messages[1..prune_end] {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("unknown");
        if let Some(content) = msg.get("content").and_then(Value::as_str) {
            let truncated = if content.len() > 200 {
                format!("{}...", &content[..200])
            } else {
                content.to_string()
            };
            summary_parts.push(format!("[{role}]: {truncated}"));
        }
    }

    let summary = format!(
        "[Context summary — {} earlier messages condensed]\n{}",
        prune_end - 1,
        summary_parts.join("\n")
    );

    // Remove the pruned messages and insert summary
    messages.drain(1..prune_end);
    messages.insert(
        1,
        serde_json::json!({
            "role": "system",
            "content": summary,
        }),
    );
}

fn find_video_url(json: &Value) -> Option<String> {
    json.get("video_url")
        .and_then(Value::as_str)
        .or_else(|| {
            json.get("video")
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
        })
        .or_else(|| json.get("url").and_then(Value::as_str))
        .or_else(|| {
            json.get("data")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("url"))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

pub fn base_system_prompt(workspace_context: &str) -> String {
    if workspace_context.trim().is_empty() {
        return "You are Grok Desktop, a concise desktop coding assistant. Respond clearly and use Markdown for code.".into();
    }
    format!(
        "You are Grok Desktop, a concise desktop coding assistant. Use the attached workspace context when it is relevant.\n\n{}",
        workspace_context
    )
}

async fn extract_error(response: reqwest::Response) -> AppResult<String> {
    let status = response.status();
    let body = response.text().await?;
    if let Ok(json) = serde_json::from_str::<Value>(&body) {
        if let Some(message) = json
            .get("error")
            .and_then(|value| value.get("message").or(Some(value)))
            .and_then(Value::as_str)
        {
            return Ok(format!("{status}: {message}"));
        }
    }
    Ok(format!("{status}: {body}"))
}

fn take_sse_event(buffer: &mut String) -> Option<String> {
    let normalized = buffer.replace("\r\n", "\n");
    *buffer = normalized;
    if let Some(index) = buffer.find("\n\n") {
        let event = buffer[..index].to_string();
        *buffer = buffer[index + 2..].to_string();
        return Some(event);
    }
    None
}
