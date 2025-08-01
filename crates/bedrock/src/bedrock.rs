mod models;

use anyhow::{Context, Error, Result, anyhow};
use aws_sdk_bedrockruntime as bedrock;
pub use aws_sdk_bedrockruntime as bedrock_client;
pub use aws_sdk_bedrockruntime::types::{
    AnyToolChoice as BedrockAnyToolChoice, AutoToolChoice as BedrockAutoToolChoice,
    ContentBlock as BedrockInnerContent, Tool as BedrockTool, ToolChoice as BedrockToolChoice,
    ToolConfiguration as BedrockToolConfig, ToolInputSchema as BedrockToolInputSchema,
    ToolSpecification as BedrockToolSpec,
};
pub use aws_smithy_types::Blob as BedrockBlob;
use aws_smithy_types::{Document, Number as AwsNumber};
pub use bedrock::operation::converse_stream::ConverseStreamInput as BedrockStreamingRequest;
pub use bedrock::types::{
    ContentBlock as BedrockRequestContent, ConversationRole as BedrockRole,
    ConverseOutput as BedrockResponse, ConverseStreamOutput as BedrockStreamingResponse,
    ImageBlock as BedrockImageBlock, Message as BedrockMessage,
    ReasoningContentBlock as BedrockThinkingBlock, ReasoningTextBlock as BedrockThinkingTextBlock,
    ResponseStream as BedrockResponseStream, ToolResultBlock as BedrockToolResultBlock,
    ToolResultContentBlock as BedrockToolResultContentBlock,
    ToolResultStatus as BedrockToolResultStatus, ToolUseBlock as BedrockToolUseBlock,
};
use futures::stream::{self, BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::HashMap;
use thiserror::Error;

pub use crate::models::*;

pub async fn stream_completion(
    client: bedrock::Client,
    request: Request,
) -> Result<BoxStream<'static, Result<BedrockStreamingResponse, BedrockError>>, Error> {
    let mut response = bedrock::Client::converse_stream(&client)
        .model_id(request.model.clone())
        .set_messages(request.messages.into());

    if let Some(Thinking::Enabled {
        budget_tokens: Some(budget_tokens),
    }) = request.thinking
    {
        let thinking_config = HashMap::from([
            ("type".to_string(), Document::String("enabled".to_string())),
            (
                "budget_tokens".to_string(),
                Document::Number(AwsNumber::PosInt(budget_tokens)),
            ),
        ]);
        response = response.additional_model_request_fields(Document::Object(HashMap::from([(
            "thinking".to_string(),
            Document::from(thinking_config),
        )])));
    }

    if request
        .tools
        .as_ref()
        .map_or(false, |t| !t.tools.is_empty())
    {
        response = response.set_tool_config(request.tools);
    }

    let output = response
        .send()
        .await
        .context("Failed to send API request to Bedrock");

    let stream = Box::pin(stream::unfold(
        output?.stream,
        move |mut stream| async move {
            match stream.recv().await {
                Ok(Some(output)) => Some((Ok(output), stream)),
                Ok(None) => None,
                Err(err) => Some((
                    Err(BedrockError::ClientError(anyhow!(
                        "{:?}",
                        aws_sdk_bedrockruntime::error::DisplayErrorContext(err)
                    ))),
                    stream,
                )),
            }
        },
    ));

    Ok(stream)
}

pub fn aws_document_to_value(document: &Document) -> Value {
    match document {
        Document::Null => Value::Null,
        Document::Bool(value) => Value::Bool(*value),
        Document::Number(value) => match *value {
            AwsNumber::PosInt(value) => Value::Number(Number::from(value)),
            AwsNumber::NegInt(value) => Value::Number(Number::from(value)),
            AwsNumber::Float(value) => Value::Number(Number::from_f64(value).unwrap()),
        },
        Document::String(value) => Value::String(value.clone()),
        Document::Array(array) => Value::Array(array.iter().map(aws_document_to_value).collect()),
        Document::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), aws_document_to_value(value)))
                .collect(),
        ),
    }
}

pub fn value_to_aws_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(value) => Document::Bool(*value),
        Value::Number(value) => {
            if let Some(value) = value.as_u64() {
                Document::Number(AwsNumber::PosInt(value))
            } else if let Some(value) = value.as_i64() {
                Document::Number(AwsNumber::NegInt(value))
            } else if let Some(value) = value.as_f64() {
                Document::Number(AwsNumber::Float(value))
            } else {
                Document::Null
            }
        }
        Value::String(value) => Document::String(value.clone()),
        Value::Array(array) => Document::Array(array.iter().map(value_to_aws_document).collect()),
        Value::Object(map) => Document::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), value_to_aws_document(value)))
                .collect(),
        ),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Thinking {
    Enabled { budget_tokens: Option<u64> },
}

#[derive(Debug)]
pub struct Request {
    pub model: String,
    pub max_tokens: u64,
    pub messages: Vec<BedrockMessage>,
    pub tools: Option<BedrockToolConfig>,
    pub thinking: Option<Thinking>,
    pub system: Option<String>,
    pub metadata: Option<Metadata>,
    pub stop_sequences: Vec<String>,
    pub temperature: Option<f32>,
    pub top_k: Option<u32>,
    pub top_p: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub user_id: Option<String>,
}

#[derive(Error, Debug)]
pub enum BedrockError {
    #[error("client error: {0}")]
    ClientError(anyhow::Error),
    #[error("extension error: {0}")]
    ExtensionError(anyhow::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
