use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CHUNK_TARGET_CHARS: usize = 1_600;
const CHUNK_OVERLAP_CHARS: usize = 220;
const MAX_DERIVED_LABEL_CHARS: usize = 512;

#[derive(Clone, Debug, Serialize)]
pub struct DocumentChunk {
    pub chunk_ref: String,
    pub ordinal: usize,
    pub heading: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub content_hash: String,
    pub estimated_tokens: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct NormalizedDocument {
    pub title: String,
    pub headings: Vec<String>,
    pub content_hash: String,
    pub chunks: Vec<DocumentChunk>,
}

pub fn normalize_document(path: &str, text: &str) -> NormalizedDocument {
    let lines: Vec<&str> = text.lines().collect();
    let mut sections = Vec::new();
    let mut section_start = 0usize;
    let mut heading = "(preamble)".to_owned();
    let mut headings = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if let Some(next_heading) = markdown_heading(line) {
            if index > section_start {
                sections.push((heading.clone(), section_start, index));
            }
            heading = bounded_label(next_heading);
            headings.push(heading.clone());
            section_start = index;
        }
    }
    if section_start < lines.len() {
        sections.push((heading, section_start, lines.len()));
    }
    if sections.is_empty() && !lines.is_empty() {
        sections.push(("(document)".to_owned(), 0, lines.len()));
    }

    let mut chunks = Vec::new();
    for (section_heading, start, end) in sections {
        split_section(path, &lines, &section_heading, start, end, &mut chunks);
    }
    for (ordinal, chunk) in chunks.iter_mut().enumerate() {
        chunk.ordinal = ordinal;
    }

    NormalizedDocument {
        title: headings
            .first()
            .cloned()
            .unwrap_or_else(|| bounded_label(path.rsplit('/').next().unwrap_or(path))),
        headings,
        content_hash: sha256_prefixed(text.as_bytes()),
        chunks,
    }
}

fn bounded_label(value: &str) -> String {
    value.chars().take(MAX_DERIVED_LABEL_CHARS).collect()
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
        Some(trimmed[hashes + 1..].trim())
    } else {
        None
    }
}

fn split_section(
    path: &str,
    lines: &[&str],
    heading: &str,
    start: usize,
    end: usize,
    chunks: &mut Vec<DocumentChunk>,
) {
    let mut cursor = start;
    while cursor < end {
        let mut next = cursor;
        let mut chars = 0usize;
        let mut has_body_content = false;
        while next < end {
            let line_chars = lines[next].chars().count() + 1;
            if next > cursor && chars + line_chars > CHUNK_TARGET_CHARS && has_body_content {
                break;
            }
            chars += line_chars;
            has_body_content |=
                !lines[next].trim().is_empty() && markdown_heading(lines[next]).is_none();
            next += 1;
        }
        if next == cursor {
            next += 1;
        }
        let content = lines[cursor..next].join("\n");
        push_bounded_chunks(path, heading, cursor + 1, next, content, chunks);
        if next >= end {
            break;
        }
        let mut overlap = 0usize;
        let mut rewind = next;
        while rewind > cursor + 1 {
            let prior = lines[rewind - 1].chars().count() + 1;
            if overlap + prior > CHUNK_OVERLAP_CHARS {
                break;
            }
            overlap += prior;
            rewind -= 1;
        }
        cursor = rewind.max(cursor + 1);
    }
}

fn push_bounded_chunks(
    path: &str,
    heading: &str,
    start_line: usize,
    end_line: usize,
    content: String,
    chunks: &mut Vec<DocumentChunk>,
) {
    if content.trim().is_empty() {
        return;
    }
    let boundaries = content
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(content.len()))
        .collect::<Vec<_>>();
    let character_count = boundaries.len().saturating_sub(1);
    if character_count <= CHUNK_TARGET_CHARS {
        push_chunk(path, heading, start_line, end_line, content, None, chunks);
        return;
    }

    let mut start_character = 0usize;
    while start_character < character_count {
        let end_character = (start_character + CHUNK_TARGET_CHARS).min(character_count);
        let start_byte = boundaries[start_character];
        let end_byte = boundaries[end_character];
        let segment = content[start_byte..end_byte].to_owned();
        if !segment.trim().is_empty() {
            let prior_newlines = content[..start_byte]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            let segment_newlines = segment.bytes().filter(|byte| *byte == b'\n').count();
            let segment_start_line = start_line + prior_newlines;
            let segment_end_line = segment_start_line + segment_newlines;
            push_chunk(
                path,
                heading,
                segment_start_line,
                segment_end_line,
                segment,
                Some((start_character, end_character)),
                chunks,
            );
        }
        if end_character == character_count {
            break;
        }
        start_character = end_character
            .saturating_sub(CHUNK_OVERLAP_CHARS)
            .max(start_character + 1);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_chunk(
    path: &str,
    heading: &str,
    start_line: usize,
    end_line: usize,
    content: String,
    character_range: Option<(usize, usize)>,
    chunks: &mut Vec<DocumentChunk>,
) {
    let identity = match character_range {
        Some((start, end)) => {
            format!("{path}\0{start_line}\0{end_line}\0chars:{start}-{end}\0{content}")
        }
        None => format!("{path}\0{start_line}\0{end_line}\0{content}"),
    };
    let digest = Sha256::digest(identity.as_bytes());
    let mut uuid_bytes = [0u8; 16];
    uuid_bytes.copy_from_slice(&digest[..16]);
    chunks.push(DocumentChunk {
        chunk_ref: format!("chunk:{}", Uuid::from_bytes(uuid_bytes)),
        ordinal: 0,
        heading: heading.to_owned(),
        start_line,
        end_line,
        estimated_tokens: estimate_tokens(&content),
        content_hash: format!("sha256:{}", hex::encode(Sha256::digest(content.as_bytes()))),
        content,
    });
}

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_preserve_headings_and_stable_line_ranges() {
        let text = "# Trip\nCurrent plan.\n\n## Booking\nConfirmed.\n";
        let document = normalize_document("Trips/example.md", text);
        assert_eq!(document.title, "Trip");
        assert_eq!(document.headings, ["Trip", "Booking"]);
        assert_eq!(document.chunks[0].start_line, 1);
        assert!(
            document
                .chunks
                .iter()
                .any(|chunk| chunk.heading == "Booking")
        );
    }

    #[test]
    fn chunks_are_deterministic() {
        let first = normalize_document("a.md", "# A\nhello");
        let second = normalize_document("a.md", "# A\nhello");
        assert_eq!(first.chunks[0].chunk_ref, second.chunks[0].chunk_ref);
        assert_eq!(first.content_hash, second.content_hash);
    }

    #[test]
    fn oversized_first_body_line_stays_with_its_heading() {
        let long_line = "current target ".repeat(180);
        let text = format!("## Current state\n\n{long_line}\n\nNext line.\n");
        let document = normalize_document("handoff.md", &text);

        assert!(document.chunks.len() > 1);
        assert!(
            document.chunks[0]
                .content
                .starts_with("## Current state\n\ncurrent target")
        );
        assert!(
            document
                .chunks
                .iter()
                .all(|chunk| chunk.content.chars().count() <= CHUNK_TARGET_CHARS)
        );
        assert!(
            document
                .chunks
                .iter()
                .all(|chunk| chunk.heading == "Current state")
        );
        assert!(
            document
                .chunks
                .last()
                .is_some_and(|chunk| chunk.content.ends_with("Next line."))
        );
    }

    #[test]
    fn oversized_heading_is_bounded_in_derived_metadata_without_changing_source_identity() {
        let heading = "H".repeat(100_000);
        let text = format!("# {heading}\ncurrent state\n");
        let document = normalize_document("large-heading.md", &text);

        assert_eq!(document.title.chars().count(), MAX_DERIVED_LABEL_CHARS);
        assert!(
            document
                .chunks
                .iter()
                .all(|chunk| chunk.heading.chars().count() <= MAX_DERIVED_LABEL_CHARS)
        );
        assert_eq!(document.content_hash, sha256_prefixed(text.as_bytes()));
        assert!(document.chunks.len() > 1);
    }

    #[test]
    fn oversized_unicode_lines_remain_within_the_embedding_safe_byte_bound() {
        let text = format!("# Map\n{}\n", "🗺".repeat(CHUNK_TARGET_CHARS * 3));
        let document = normalize_document("map.md", &text);

        assert!(document.chunks.len() > 3);
        assert!(
            document
                .chunks
                .iter()
                .all(|chunk| chunk.content.chars().count() <= CHUNK_TARGET_CHARS)
        );
        assert!(
            document
                .chunks
                .iter()
                .all(|chunk| chunk.content.len() <= CHUNK_TARGET_CHARS * 4)
        );
    }
}
