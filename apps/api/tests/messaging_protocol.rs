#[path = "../src/messaging_protocol.rs"]
mod messaging_protocol;

const _: &str = messaging_protocol::CONTINUATION_SYSTEM_BODY;

use chrono::{TimeZone, Utc};
use messaging_protocol::{
    CanonicalMessage, ConversationHeader, ConversationKind, ConversationParticipant,
    ConversationStatus, MessageKind, MessageRef, ProtocolError, SendMessageInput,
    conversation_id_from_path, conversation_metadata, conversation_path, is_conversation_candidate,
    is_workspace_import, parse_conversation, render_conversation, request_hash,
    request_hash_with_reply_target, validate_conversation_entry, validate_send_input,
};
use uuid::Uuid;

fn as_of() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 27, 8, 0, 0).single().unwrap()
}

fn input() -> SendMessageInput {
    SendMessageInput {
        client_key: "01J00000000000000000000000".to_owned(),
        kind: MessageKind::Question,
        body_md: "Can you verify this?".to_owned(),
        refs: vec![MessageRef {
            entry_ref: Some("entry:018f0000-0000-7000-8000-000000000001".to_owned()),
            url: None,
            label: Some("Evidence".to_owned()),
        }],
        in_reply_to: None,
        correlation_id: Some("task:check".to_owned()),
        expects_reply: true,
        reply_by: Some(as_of() + chrono::Duration::minutes(10)),
    }
}

fn header() -> ConversationHeader {
    ConversationHeader {
        schema: "conversation.v1".to_owned(),
        conversation_id: Uuid::parse_str("018f0000-0000-7000-8000-000000000010").unwrap(),
        conversation_kind: ConversationKind::Direct,
        direct_key: None,
        subject: Some("Release check".to_owned()),
        status: ConversationStatus::Open,
        participants: vec![
            ConversationParticipant {
                agent_id: "echo".to_owned(),
                role: "participant".to_owned(),
            },
            ConversationParticipant {
                agent_id: "owner".to_owned(),
                role: "participant".to_owned(),
            },
        ],
        created_by_agent_id: "owner".to_owned(),
        continues_from: None,
        agent_streak: 0,
        needs_human: false,
        latest_sync_cursor: 42,
        created_at: as_of(),
        closed_at: None,
    }
}

fn messages() -> Vec<CanonicalMessage> {
    let mut messages = vec![
        CanonicalMessage {
            seq: 1,
            message_id: Uuid::parse_str("018f0000-0000-7000-8000-000000000011").unwrap(),
            from_agent_id: Some("owner".to_owned()),
            client_key: Some("01J00000000000000000000000".to_owned()),
            system_key: None,
            request_hash: None,
            kind: MessageKind::Question,
            body_md: "Arbitrary Markdown\n\n<!-- /brunn-message-v1 -->\n`-->` 🛰️".to_owned(),
            refs: input().refs,
            in_reply_to_conversation_id: None,
            in_reply_to: None,
            correlation_id: Some("has-->marker".to_owned()),
            expects_reply: true,
            reply_by: Some(as_of() + chrono::Duration::minutes(10)),
            reply_by_handled_at: None,
            sync_cursor: 41,
            created_at: as_of(),
        },
        CanonicalMessage {
            seq: 2,
            message_id: Uuid::parse_str("018f0000-0000-7000-8000-000000000012").unwrap(),
            from_agent_id: Some("echo".to_owned()),
            client_key: Some("01J00000000000000000000001".to_owned()),
            system_key: None,
            request_hash: None,
            kind: MessageKind::Text,
            body_md: "Verified.".to_owned(),
            refs: vec![],
            in_reply_to_conversation_id: Some(header().conversation_id),
            in_reply_to: Some(1),
            correlation_id: None,
            expects_reply: false,
            reply_by: None,
            reply_by_handled_at: None,
            sync_cursor: 42,
            created_at: as_of() + chrono::Duration::seconds(2),
        },
    ];
    for message in &mut messages {
        refresh_request_hash(message);
    }
    messages
}

fn refresh_request_hash(message: &mut CanonicalMessage) {
    if message.kind == MessageKind::System {
        return;
    }
    message.request_hash = Some(request_hash_with_reply_target(
        header().conversation_id,
        message.in_reply_to_conversation_id,
        &SendMessageInput {
            client_key: message.client_key.clone().unwrap(),
            kind: message.kind,
            body_md: message.body_md.clone(),
            refs: message.refs.clone(),
            in_reply_to: message.in_reply_to,
            correlation_id: message.correlation_id.clone(),
            expects_reply: message.expects_reply,
            reply_by: message.reply_by,
        },
    ));
}

#[test]
fn canonical_conversation_round_trips_arbitrary_markdown() {
    assert_eq!(
        conversation_path(header().conversation_id),
        ".brunn/conversations/018f0000-0000-7000-8000-000000000010.md"
    );
    let rendered = render_conversation(&header(), &messages()).unwrap();
    assert!(rendered.contains("\\u003e"));
    let (parsed_header, parsed_messages) = parse_conversation(&rendered).unwrap();
    assert_eq!(parsed_header, header());
    assert_eq!(parsed_messages, messages());
    assert_eq!(
        render_conversation(&parsed_header, &parsed_messages).unwrap(),
        rendered
    );
}

#[test]
fn typed_metadata_and_path_must_match_the_canonical_header() {
    let header = header();
    let rendered = render_conversation(&header, &messages()).unwrap();
    let metadata = conversation_metadata(&header);
    let validated = validate_conversation_entry(
        &conversation_path(header.conversation_id),
        &metadata,
        &rendered,
    )
    .unwrap()
    .expect("typed conversation is managed");
    assert_eq!(validated.0, header);

    let wrapped = serde_json::json!({"client": metadata, "server": {"ignored": true}});
    assert!(
        validate_conversation_entry(
            &conversation_path(header.conversation_id),
            &wrapped,
            &rendered,
        )
        .unwrap()
        .is_some()
    );

    let mut wrong_metadata = conversation_metadata(&header);
    wrong_metadata["conversation"]["latest_sync_cursor"] = serde_json::json!(41);
    assert!(matches!(
        validate_conversation_entry(
            &conversation_path(header.conversation_id),
            &wrong_metadata,
            &rendered,
        ),
        Err(ProtocolError::Invalid(message)) if message.contains("metadata")
    ));
    assert!(
        validate_conversation_entry("Notes/ordinary.md", &serde_json::json!({}), "hello")
            .unwrap()
            .is_none()
    );
    let imported = serde_json::json!({
        "client": conversation_metadata(&header),
        "_brunn_import": {
            "format": messaging_protocol::WORKSPACE_IMPORT_FORMAT
        }
    });
    assert!(is_conversation_candidate(
        &conversation_path(header.conversation_id),
        &imported
    ));
    assert!(is_workspace_import(&imported));
    assert!(!is_workspace_import(&wrapped));
    assert!(conversation_id_from_path(&conversation_path(Uuid::new_v4())).is_none());
}

#[test]
fn direct_identity_separates_default_and_subject_conversations() {
    let subject_conversation = header();
    render_conversation(&subject_conversation, &messages()).unwrap();

    let mut default_conversation = header();
    default_conversation.subject = None;
    default_conversation.direct_key = Some("echo|owner".to_owned());
    render_conversation(&default_conversation, &messages()).unwrap();

    let mut missing_default_key = default_conversation.clone();
    missing_default_key.direct_key = None;
    assert!(matches!(
        render_conversation(&missing_default_key, &messages()),
        Err(ProtocolError::Invalid(message)) if message.contains("requires direct_key")
    ));

    let mut keyed_subject = subject_conversation;
    keyed_subject.direct_key = Some("echo|owner".to_owned());
    assert!(matches!(
        render_conversation(&keyed_subject, &messages()),
        Err(ProtocolError::Invalid(message)) if message.contains("cannot contain direct_key")
    ));
}

#[test]
fn canonical_parser_rejects_body_length_and_sequence_tampering() {
    let rendered = render_conversation(&header(), &messages()).unwrap();
    let mut wrong_length = rendered.clone();
    let length_prefix = "\"body_bytes\":";
    let length_start = wrong_length.find(length_prefix).unwrap() + length_prefix.len();
    let length_end = length_start
        + wrong_length[length_start..]
            .find(|character: char| !character.is_ascii_digit())
            .unwrap();
    let body_bytes = wrong_length[length_start..length_end]
        .parse::<usize>()
        .unwrap();
    wrong_length.replace_range(length_start..length_end, &(body_bytes - 1).to_string());
    assert_eq!(
        parse_conversation(&wrong_length),
        Err(ProtocolError::NonCanonical)
    );

    let wrong_sequence = rendered.replacen("\"seq\":2", "\"seq\":3", 1);
    assert!(matches!(
        parse_conversation(&wrong_sequence),
        Err(ProtocolError::Invalid(message)) if message.contains("gapless")
    ));

    let mut stale_hash = messages();
    stale_hash[0].body_md.push_str(" changed");
    assert!(matches!(
        render_conversation(&header(), &stale_hash),
        Err(ProtocolError::Invalid(message)) if message.contains("request_hash")
    ));

    let mut outsider = messages();
    outsider[0].from_agent_id = Some("not-a-participant".to_owned());
    assert!(matches!(
        render_conversation(&header(), &outsider),
        Err(ProtocolError::Invalid(message)) if message.contains("participant")
    ));

    let mut stale_cursor = header();
    stale_cursor.latest_sync_cursor = 41;
    assert!(matches!(
        render_conversation(&stale_cursor, &messages()),
        Err(ProtocolError::Invalid(message)) if message.contains("latest_sync_cursor")
    ));

    let mut control_correlation = messages();
    control_correlation[0].correlation_id = Some("release\nspoofed".to_owned());
    refresh_request_hash(&mut control_correlation[0]);
    assert!(matches!(
        render_conversation(&header(), &control_correlation),
        Err(ProtocolError::Invalid(message)) if message.contains("correlation_id")
    ));
}

#[test]
fn canonical_replies_carry_the_owning_conversation_across_rollover() {
    let predecessor = Uuid::parse_str("018f0000-0000-7000-8000-000000000009").unwrap();
    let mut continued_header = header();
    continued_header.continues_from = Some(predecessor);
    let mut continued_messages = messages();
    continued_messages[1].in_reply_to_conversation_id = Some(predecessor);
    continued_messages[1].in_reply_to = Some(500);
    refresh_request_hash(&mut continued_messages[1]);
    let rendered = render_conversation(&continued_header, &continued_messages).unwrap();
    let (_, parsed) = parse_conversation(&rendered).unwrap();
    assert_eq!(parsed[1].in_reply_to_conversation_id, Some(predecessor));
    assert_eq!(parsed[1].in_reply_to, Some(500));

    let mut missing_owner = continued_messages.clone();
    missing_owner[1].in_reply_to_conversation_id = None;
    assert!(matches!(
        render_conversation(&continued_header, &missing_owner),
        Err(ProtocolError::Invalid(message)) if message.contains("in_reply_to")
    ));

    let mut forward_reference = messages();
    forward_reference[1].in_reply_to = Some(2);
    assert!(matches!(
        render_conversation(&header(), &forward_reference),
        Err(ProtocolError::Invalid(message)) if message.contains("in_reply_to")
    ));
}

#[test]
fn client_input_is_strict_and_question_deadline_is_bounded() {
    let valid = input();
    validate_send_input(&valid, as_of()).unwrap();

    let mut unknown = serde_json::to_value(&valid).unwrap();
    unknown["from"] = serde_json::json!("spoofed");
    assert!(serde_json::from_value::<SendMessageInput>(unknown).is_err());

    let mut bad_key = input();
    bad_key.client_key = "new-key-per-retry".to_owned();
    assert!(validate_send_input(&bad_key, as_of()).is_err());

    let mut overflow_key = input();
    overflow_key.client_key = "81J00000000000000000000000".to_owned();
    assert!(
        validate_send_input(&overflow_key, as_of()).is_err(),
        "a 128-bit ULID cannot begin above 7"
    );

    let mut late = input();
    late.reply_by = Some(as_of() + chrono::Duration::hours(25));
    assert!(validate_send_input(&late, as_of()).is_err());

    let mut oversized = input();
    oversized.body_md = "é".repeat(8_193);
    assert!(validate_send_input(&oversized, as_of()).is_err());
}

#[test]
fn request_hash_covers_target_and_payload_but_is_retry_stable() {
    let conversation = header().conversation_id;
    let original = input();
    assert_eq!(
        request_hash(conversation, &original),
        request_hash(conversation, &original)
    );

    let other_conversation = Uuid::parse_str("018f0000-0000-7000-8000-000000000020").unwrap();
    assert_ne!(
        request_hash(conversation, &original),
        request_hash(other_conversation, &original)
    );

    let mut changed = input();
    changed.body_md.push('!');
    assert_ne!(
        request_hash(conversation, &original),
        request_hash(conversation, &changed)
    );

    let mut reply = input();
    reply.in_reply_to = Some(1);
    assert_ne!(
        request_hash_with_reply_target(conversation, Some(conversation), &reply),
        request_hash_with_reply_target(conversation, Some(other_conversation), &reply),
        "the server-derived owning conversation is part of replay identity"
    );
}
