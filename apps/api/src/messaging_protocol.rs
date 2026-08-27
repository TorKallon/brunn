use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

pub const MESSAGE_BODY_LIMIT_BYTES: usize = 16 * 1024;
pub const MESSAGE_REFS_LIMIT: usize = 32;
pub const SUBJECT_LIMIT_CHARS: usize = 240;
pub const CORRELATION_ID_LIMIT_CHARS: usize = 200;
pub const MAX_MESSAGES_PER_CONVERSATION: i64 = 500;

const HEADER_PREFIX: &str = "<!-- straylight-conversation-v1 ";
const MESSAGE_PREFIX: &str = "<!-- straylight-message-v1 ";
const COMMENT_SUFFIX: &str = " -->\n";
const MESSAGE_END: &str = "\n<!-- /straylight-message-v1 -->\n";

static AGENT_ID: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z0-9]+(?:[._-][a-z0-9]+)*$").expect("messaging agent id regex"));
static CLIENT_KEY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9A-HJKMNP-TV-Z]{26}$").expect("Crockford ULID regex"));

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("{0}")]
    Invalid(String),
    #[error("stored conversation is not canonical Markdown")]
    NonCanonical,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Direct,
    Group,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStatus {
    Open,
    PausedForHuman,
    Closed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    Question,
    System,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageInput {
    pub client_key: String,
    #[serde(default = "default_message_kind")]
    pub kind: MessageKind,
    pub body_md: String,
    #[serde(default)]
    pub refs: Vec<MessageRef>,
    #[serde(default)]
    pub in_reply_to: Option<i64>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub expects_reply: bool,
    #[serde(default)]
    pub reply_by: Option<DateTime<Utc>>,
}

fn default_message_kind() -> MessageKind {
    MessageKind::Text
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationParticipant {
    pub agent_id: String,
    pub role: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationHeader {
    pub schema: String,
    pub conversation_id: Uuid,
    pub conversation_kind: ConversationKind,
    pub direct_key: Option<String>,
    pub subject: Option<String>,
    pub status: ConversationStatus,
    pub participants: Vec<ConversationParticipant>,
    pub created_by_agent_id: String,
    pub continues_from: Option<Uuid>,
    pub agent_streak: i32,
    pub needs_human: bool,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMessage {
    pub seq: i64,
    pub message_id: Uuid,
    pub from_agent_id: Option<String>,
    pub client_key: Option<String>,
    pub system_key: Option<String>,
    pub request_hash: Option<String>,
    pub kind: MessageKind,
    pub body_md: String,
    #[serde(default)]
    pub refs: Vec<MessageRef>,
    pub in_reply_to: Option<i64>,
    pub correlation_id: Option<String>,
    pub expects_reply: bool,
    pub reply_by: Option<DateTime<Utc>>,
    pub reply_by_handled_at: Option<DateTime<Utc>>,
    pub sync_cursor: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MessageEnvelope {
    seq: i64,
    message_id: Uuid,
    from_agent_id: Option<String>,
    client_key: Option<String>,
    system_key: Option<String>,
    request_hash: Option<String>,
    kind: MessageKind,
    #[serde(default)]
    refs: Vec<MessageRef>,
    in_reply_to: Option<i64>,
    correlation_id: Option<String>,
    expects_reply: bool,
    reply_by: Option<DateTime<Utc>>,
    reply_by_handled_at: Option<DateTime<Utc>>,
    sync_cursor: i64,
    created_at: DateTime<Utc>,
    body_bytes: usize,
}

impl From<&CanonicalMessage> for MessageEnvelope {
    fn from(message: &CanonicalMessage) -> Self {
        Self {
            seq: message.seq,
            message_id: message.message_id,
            from_agent_id: message.from_agent_id.clone(),
            client_key: message.client_key.clone(),
            system_key: message.system_key.clone(),
            request_hash: message.request_hash.clone(),
            kind: message.kind,
            refs: message.refs.clone(),
            in_reply_to: message.in_reply_to,
            correlation_id: message.correlation_id.clone(),
            expects_reply: message.expects_reply,
            reply_by: message.reply_by,
            reply_by_handled_at: message.reply_by_handled_at,
            sync_cursor: message.sync_cursor,
            created_at: message.created_at,
            body_bytes: message.body_md.len(),
        }
    }
}

pub fn conversation_path(conversation_id: Uuid) -> String {
    format!(".straylight/conversations/{conversation_id}.md")
}

pub fn validate_agent_id(agent_id: &str) -> Result<(), ProtocolError> {
    if !(1..=80).contains(&agent_id.len()) || !AGENT_ID.is_match(agent_id) {
        return Err(ProtocolError::Invalid(
            "agent_id must be 1 to 80 lowercase letters or numbers separated by '.', '_', or '-'"
                .to_owned(),
        ));
    }
    Ok(())
}

pub fn validate_subject(subject: Option<&str>) -> Result<Option<String>, ProtocolError> {
    let Some(subject) = subject else {
        return Ok(None);
    };
    let trimmed = subject.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > SUBJECT_LIMIT_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(ProtocolError::Invalid(format!(
            "subject must be a printable single line of at most {SUBJECT_LIMIT_CHARS} characters"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

pub fn validate_send_input(
    input: &SendMessageInput,
    as_of: DateTime<Utc>,
) -> Result<(), ProtocolError> {
    if !CLIENT_KEY.is_match(&input.client_key) {
        return Err(ProtocolError::Invalid(
            "client_key must be a 26-character Crockford ULID and reused unchanged for retries"
                .to_owned(),
        ));
    }
    validate_body(&input.body_md)?;
    validate_refs(&input.refs)?;
    if input.in_reply_to.is_some_and(|seq| seq <= 0) {
        return Err(ProtocolError::Invalid(
            "in_reply_to must be a positive conversation sequence".to_owned(),
        ));
    }
    if let Some(value) = input.correlation_id.as_deref() {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > CORRELATION_ID_LIMIT_CHARS
            || trimmed.chars().any(char::is_control)
        {
            return Err(ProtocolError::Invalid(format!(
                "correlation_id must be a printable single line of at most {CORRELATION_ID_LIMIT_CHARS} characters"
            )));
        }
    }
    if input.kind == MessageKind::System {
        return Err(ProtocolError::Invalid(
            "clients cannot send system messages".to_owned(),
        ));
    }
    if input.expects_reply && input.kind != MessageKind::Question {
        return Err(ProtocolError::Invalid(
            "expects_reply is allowed only for kind question".to_owned(),
        ));
    }
    match input.reply_by {
        Some(_) if !input.expects_reply => Err(ProtocolError::Invalid(
            "reply_by requires expects_reply".to_owned(),
        )),
        Some(reply_by) if reply_by <= as_of || reply_by > as_of + chrono::Duration::hours(24) => {
            Err(ProtocolError::Invalid(
                "reply_by must be after now and no more than 24 hours away".to_owned(),
            ))
        }
        _ => Ok(()),
    }
}

pub fn validate_body(body_md: &str) -> Result<(), ProtocolError> {
    if body_md.is_empty() || body_md.len() > MESSAGE_BODY_LIMIT_BYTES {
        return Err(ProtocolError::Invalid(format!(
            "body_md must contain 1 to {MESSAGE_BODY_LIMIT_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

pub fn validate_refs(refs: &[MessageRef]) -> Result<(), ProtocolError> {
    if refs.len() > MESSAGE_REFS_LIMIT {
        return Err(ProtocolError::Invalid(format!(
            "refs exceeds the limit of {MESSAGE_REFS_LIMIT} entries"
        )));
    }
    for reference in refs {
        if reference.label.as_ref().is_some_and(|label| {
            label.trim().is_empty()
                || label.chars().count() > SUBJECT_LIMIT_CHARS
                || label.chars().any(char::is_control)
        }) {
            return Err(ProtocolError::Invalid(
                "ref label must be a printable single line of at most 240 characters".to_owned(),
            ));
        }
        match (reference.entry_ref.as_deref(), reference.url.as_deref()) {
            (Some(entry_ref), None) => {
                let raw = entry_ref.strip_prefix("entry:").ok_or_else(|| {
                    ProtocolError::Invalid("ref entry_ref must use entry:<uuid>".to_owned())
                })?;
                Uuid::parse_str(raw).map_err(|_| {
                    ProtocolError::Invalid("ref entry_ref must use entry:<uuid>".to_owned())
                })?;
            }
            (None, Some(url)) => {
                let parsed = Url::parse(url).map_err(|_| {
                    ProtocolError::Invalid("ref url must be an absolute HTTP(S) URL".to_owned())
                })?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(ProtocolError::Invalid(
                        "ref url must be an absolute HTTP(S) URL without credentials".to_owned(),
                    ));
                }
            }
            _ => {
                return Err(ProtocolError::Invalid(
                    "each ref must contain exactly one of entry_ref or url".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub fn render_conversation(
    header: &ConversationHeader,
    messages: &[CanonicalMessage],
) -> Result<String, ProtocolError> {
    validate_header(header)?;
    validate_messages(messages)?;
    let mut output = String::new();
    output.push_str(HEADER_PREFIX);
    output.push_str(&comment_safe_json(header)?);
    output.push_str(COMMENT_SUFFIX);
    for message in messages {
        output.push_str(MESSAGE_PREFIX);
        output.push_str(&comment_safe_json(&MessageEnvelope::from(message))?);
        output.push_str(COMMENT_SUFFIX);
        output.push_str(&message.body_md);
        output.push_str(MESSAGE_END);
    }
    Ok(output)
}

pub fn parse_conversation(
    markdown: &str,
) -> Result<(ConversationHeader, Vec<CanonicalMessage>), ProtocolError> {
    let (header_json, mut offset) = parse_comment(markdown, 0, HEADER_PREFIX)?;
    let header: ConversationHeader =
        serde_json::from_str(header_json).map_err(|_| ProtocolError::NonCanonical)?;
    let mut messages = Vec::new();
    while offset < markdown.len() {
        let (message_json, body_offset) = parse_comment(markdown, offset, MESSAGE_PREFIX)?;
        let envelope: MessageEnvelope =
            serde_json::from_str(message_json).map_err(|_| ProtocolError::NonCanonical)?;
        let body_end = body_offset
            .checked_add(envelope.body_bytes)
            .filter(|end| *end <= markdown.len())
            .ok_or(ProtocolError::NonCanonical)?;
        if !markdown.is_char_boundary(body_end) || !markdown[body_end..].starts_with(MESSAGE_END) {
            return Err(ProtocolError::NonCanonical);
        }
        let body_md = markdown[body_offset..body_end].to_owned();
        messages.push(CanonicalMessage {
            seq: envelope.seq,
            message_id: envelope.message_id,
            from_agent_id: envelope.from_agent_id,
            client_key: envelope.client_key,
            system_key: envelope.system_key,
            request_hash: envelope.request_hash,
            kind: envelope.kind,
            body_md,
            refs: envelope.refs,
            in_reply_to: envelope.in_reply_to,
            correlation_id: envelope.correlation_id,
            expects_reply: envelope.expects_reply,
            reply_by: envelope.reply_by,
            reply_by_handled_at: envelope.reply_by_handled_at,
            sync_cursor: envelope.sync_cursor,
            created_at: envelope.created_at,
        });
        offset = body_end + MESSAGE_END.len();
    }
    validate_header(&header)?;
    validate_messages(&messages)?;
    if render_conversation(&header, &messages)? != markdown {
        return Err(ProtocolError::NonCanonical);
    }
    Ok((header, messages))
}

pub fn request_hash(conversation_id: Uuid, input: &SendMessageInput) -> String {
    #[derive(Serialize)]
    struct RequestHash<'a> {
        conversation_id: Uuid,
        client_key: &'a str,
        kind: MessageKind,
        body_md: &'a str,
        refs: &'a [MessageRef],
        in_reply_to: Option<i64>,
        correlation_id: &'a Option<String>,
        expects_reply: bool,
        reply_by: Option<DateTime<Utc>>,
    }
    let bytes = serde_json::to_vec(&RequestHash {
        conversation_id,
        client_key: &input.client_key,
        kind: input.kind,
        body_md: &input.body_md,
        refs: &input.refs,
        in_reply_to: input.in_reply_to,
        correlation_id: &input.correlation_id,
        expects_reply: input.expects_reply,
        reply_by: input.reply_by,
    })
    .expect("messaging request hash payload is serializable");
    hex::encode(Sha256::digest(bytes))
}

fn validate_header(header: &ConversationHeader) -> Result<(), ProtocolError> {
    if header.schema != "conversation.v1" {
        return Err(ProtocolError::Invalid(
            "conversation schema must be conversation.v1".to_owned(),
        ));
    }
    validate_subject(header.subject.as_deref())?;
    validate_agent_id(&header.created_by_agent_id)?;
    match header.conversation_kind {
        ConversationKind::Direct => {
            let direct_key = header.direct_key.as_deref().ok_or_else(|| {
                ProtocolError::Invalid("direct conversation requires direct_key".to_owned())
            })?;
            if direct_key.trim().is_empty() || direct_key.chars().count() > 200 {
                return Err(ProtocolError::Invalid(
                    "direct_key must contain 1 to 200 characters".to_owned(),
                ));
            }
        }
        ConversationKind::Group if header.direct_key.is_some() => {
            return Err(ProtocolError::Invalid(
                "group conversation cannot contain direct_key".to_owned(),
            ));
        }
        ConversationKind::Group => {}
    }
    if !(0..=20).contains(&header.agent_streak) {
        return Err(ProtocolError::Invalid(
            "agent_streak must be between 0 and 20".to_owned(),
        ));
    }
    if header.status == ConversationStatus::PausedForHuman && !header.needs_human {
        return Err(ProtocolError::Invalid(
            "paused conversation must need human attention".to_owned(),
        ));
    }
    if (header.status == ConversationStatus::Closed) != header.closed_at.is_some() {
        return Err(ProtocolError::Invalid(
            "closed_at must be present exactly when a conversation is closed".to_owned(),
        ));
    }
    if header.participants.len() < 2 {
        return Err(ProtocolError::Invalid(
            "conversation must have at least two participants".to_owned(),
        ));
    }
    let mut previous: Option<&str> = None;
    for participant in &header.participants {
        validate_agent_id(&participant.agent_id)?;
        if !matches!(participant.role.as_str(), "participant" | "observer") {
            return Err(ProtocolError::Invalid(
                "participant role must be participant or observer".to_owned(),
            ));
        }
        if previous.is_some_and(|value| value >= participant.agent_id.as_str()) {
            return Err(ProtocolError::Invalid(
                "participants must be unique and sorted by agent_id".to_owned(),
            ));
        }
        previous = Some(&participant.agent_id);
    }
    if header.continues_from == Some(header.conversation_id) {
        return Err(ProtocolError::Invalid(
            "conversation cannot continue from itself".to_owned(),
        ));
    }
    Ok(())
}

fn validate_messages(messages: &[CanonicalMessage]) -> Result<(), ProtocolError> {
    if messages.len() > MAX_MESSAGES_PER_CONVERSATION as usize {
        return Err(ProtocolError::Invalid(format!(
            "conversation exceeds {MAX_MESSAGES_PER_CONVERSATION} messages"
        )));
    }
    for (index, message) in messages.iter().enumerate() {
        let expected = index as i64 + 1;
        if message.seq != expected {
            return Err(ProtocolError::Invalid(
                "conversation message sequences must be gapless from 1".to_owned(),
            ));
        }
        validate_body(&message.body_md)?;
        validate_refs(&message.refs)?;
        if message.sync_cursor <= 0
            || index > 0 && message.sync_cursor <= messages[index - 1].sync_cursor
        {
            return Err(ProtocolError::Invalid(
                "message sync cursors must be positive and increasing".to_owned(),
            ));
        }
        if message
            .in_reply_to
            .is_some_and(|seq| seq <= 0 || seq >= message.seq)
        {
            return Err(ProtocolError::Invalid(
                "in_reply_to must name an earlier conversation sequence".to_owned(),
            ));
        }
        match message.kind {
            MessageKind::System => {
                if message.from_agent_id.is_some()
                    || message.client_key.is_some()
                    || message.request_hash.is_some()
                    || message.system_key.as_deref().is_none_or(|key| {
                        key.trim().is_empty() || key.chars().count() > 200 || key != key.trim()
                    })
                {
                    return Err(ProtocolError::Invalid(
                        "system message identity and dedupe fields are invalid".to_owned(),
                    ));
                }
            }
            MessageKind::Text | MessageKind::Question => {
                if message.system_key.is_some() {
                    return Err(ProtocolError::Invalid(
                        "non-system message cannot contain system_key".to_owned(),
                    ));
                }
                let sender = message.from_agent_id.as_deref().ok_or_else(|| {
                    ProtocolError::Invalid("non-system message requires a sender".to_owned())
                })?;
                validate_agent_id(sender)?;
                let client_key = message.client_key.as_deref().ok_or_else(|| {
                    ProtocolError::Invalid("non-system message requires a client_key".to_owned())
                })?;
                if !CLIENT_KEY.is_match(client_key) {
                    return Err(ProtocolError::Invalid(
                        "stored client_key must be a Crockford ULID".to_owned(),
                    ));
                }
                let request_hash = message.request_hash.as_deref().ok_or_else(|| {
                    ProtocolError::Invalid("non-system message requires request_hash".to_owned())
                })?;
                if request_hash.len() != 64
                    || !request_hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(ProtocolError::Invalid(
                        "stored request_hash must be lowercase SHA-256 hex".to_owned(),
                    ));
                }
            }
        }
        if message.expects_reply && message.kind != MessageKind::Question {
            return Err(ProtocolError::Invalid(
                "expects_reply is allowed only for kind question".to_owned(),
            ));
        }
        if message.reply_by.is_some() && !message.expects_reply {
            return Err(ProtocolError::Invalid(
                "reply_by requires expects_reply".to_owned(),
            ));
        }
        if message.reply_by_handled_at.is_some() && message.reply_by.is_none() {
            return Err(ProtocolError::Invalid(
                "reply_by_handled_at requires reply_by".to_owned(),
            ));
        }
    }
    Ok(())
}

fn comment_safe_json(value: &impl Serialize) -> Result<String, ProtocolError> {
    serde_json::to_string(value)
        .map(|json| json.replace('>', "\\u003e"))
        .map_err(|_| ProtocolError::NonCanonical)
}

fn parse_comment<'a>(
    markdown: &'a str,
    offset: usize,
    prefix: &str,
) -> Result<(&'a str, usize), ProtocolError> {
    let tail = markdown.get(offset..).ok_or(ProtocolError::NonCanonical)?;
    if !tail.starts_with(prefix) {
        return Err(ProtocolError::NonCanonical);
    }
    let json_start = offset + prefix.len();
    let json_tail = markdown
        .get(json_start..)
        .ok_or(ProtocolError::NonCanonical)?;
    let relative_end = json_tail
        .find(COMMENT_SUFFIX)
        .ok_or(ProtocolError::NonCanonical)?;
    let json_end = json_start + relative_end;
    Ok((
        &markdown[json_start..json_end],
        json_end + COMMENT_SUFFIX.len(),
    ))
}
