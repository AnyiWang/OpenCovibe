use futures_util::StreamExt;
use reqwest::Client;
use serde_json::Value;
use tokio::sync::mpsc;

const DEFAULT_API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

/// SSE event parsed from the Anthropic streaming API
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// message_start — contains message metadata
    MessageStart { message: Value },
    /// content_block_start — new content block (text or tool_use)
    ContentBlockStart { index: usize, content_block: Value },
    /// content_block_delta — incremental content
    ContentBlockDelta { index: usize, delta: Value },
    /// content_block_stop — block finished
    ContentBlockStop { index: usize },
    /// message_delta — message-level updates (stop_reason, usage)
    MessageDelta { delta: Value, usage: Option<Value> },
    /// message_stop — stream complete
    MessageStop,
    /// Error from API
    Error { error: Value },
}

/// Send a streaming request to the Anthropic Messages API
pub async fn stream_messages(
    api_key: &str,
    model: &str,
    messages: Vec<Value>,
    tools: Vec<Value>,
    system_prompt: Option<&str>,
    max_tokens: u32,
    base_url: Option<&str>,
) -> Result<mpsc::Receiver<StreamEvent>, String> {
    let client = Client::new();

    let url = match base_url {
        Some(u) if !u.is_empty() => {
            let base = u.trim_end_matches('/');
            if base.ends_with("/v1/messages") {
                base.to_string()
            } else if base.ends_with("/v1") {
                format!("{}/messages", base)
            } else {
                format!("{}/v1/messages", base)
            }
        }
        _ => DEFAULT_API_URL.to_string(),
    };

    log::debug!(
        "[anthropic] POST {} model={} messages={} tools={}",
        url,
        model,
        messages.len(),
        tools.len()
    );

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }

    if let Some(sys) = system_prompt {
        body["system"] = Value::String(sys.to_string());
    }

    let response = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("API request failed: {}", e))?;

    let status = response.status();
    log::debug!("[anthropic] response status: {}", status);

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        log::error!("[anthropic] error body: {}", body);
        return Err(format!("API error {}: {}", status, body));
    }

    let (tx, rx) = mpsc::channel::<StreamEvent>(256);

    // Parse SSE stream in background
    tokio::spawn(async move {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut current_event_type = String::new();
        let mut current_data = String::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(_) => break,
            };

            let text = String::from_utf8_lossy(&chunk);
            buffer.push_str(&text);

            // Process complete SSE messages
            while let Some(pos) = buffer.find("\n\n") {
                let message = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                for line in message.lines() {
                    if let Some(event_type) = line.strip_prefix("event: ") {
                        current_event_type = event_type.trim().to_string();
                    } else if let Some(data) = line.strip_prefix("data: ") {
                        current_data.push_str(data.trim());
                    }
                }

                if !current_event_type.is_empty() && !current_data.is_empty() {
                    if let Ok(data) = serde_json::from_str::<Value>(&current_data) {
                        let event = match current_event_type.as_str() {
                            "message_start" => Some(StreamEvent::MessageStart {
                                message: data.get("message").cloned().unwrap_or(Value::Null),
                            }),
                            "content_block_start" => Some(StreamEvent::ContentBlockStart {
                                index: data.get("index").and_then(|v| v.as_u64()).unwrap_or(0)
                                    as usize,
                                content_block: data
                                    .get("content_block")
                                    .cloned()
                                    .unwrap_or(Value::Null),
                            }),
                            "content_block_delta" => Some(StreamEvent::ContentBlockDelta {
                                index: data.get("index").and_then(|v| v.as_u64()).unwrap_or(0)
                                    as usize,
                                delta: data.get("delta").cloned().unwrap_or(Value::Null),
                            }),
                            "content_block_stop" => Some(StreamEvent::ContentBlockStop {
                                index: data.get("index").and_then(|v| v.as_u64()).unwrap_or(0)
                                    as usize,
                            }),
                            "message_delta" => Some(StreamEvent::MessageDelta {
                                delta: data.get("delta").cloned().unwrap_or(Value::Null),
                                usage: data.get("usage").cloned(),
                            }),
                            "message_stop" => Some(StreamEvent::MessageStop),
                            "error" => Some(StreamEvent::Error {
                                error: data.get("error").cloned().unwrap_or(data),
                            }),
                            "ping" => None,
                            _ => None,
                        };

                        if let Some(evt) = event {
                            if tx.send(evt).await.is_err() {
                                return; // receiver dropped
                            }
                        }
                    }
                }

                current_event_type.clear();
                current_data.clear();
            }
        }
    });

    Ok(rx)
}
