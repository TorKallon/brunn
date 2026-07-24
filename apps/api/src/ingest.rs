use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CHUNK_TARGET_CHARS: usize = 1_600;
const CHUNK_OVERLAP_CHARS: usize = 220;

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
            heading = next_heading.to_owned();
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
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned()),
        headings,
        content_hash: sha256_prefixed(text.as_bytes()),
        chunks,
    }
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
        if !content.trim().is_empty() {
            let digest =
                Sha256::digest(format!("{path}\0{}\0{}\0{content}", cursor + 1, next).as_bytes());
            let mut uuid_bytes = [0u8; 16];
            uuid_bytes.copy_from_slice(&digest[..16]);
            chunks.push(DocumentChunk {
                chunk_ref: format!("chunk:{}", Uuid::from_bytes(uuid_bytes)),
                ordinal: 0,
                heading: heading.to_owned(),
                start_line: cursor + 1,
                end_line: next,
                estimated_tokens: estimate_tokens(&content),
                content_hash: format!("sha256:{}", hex::encode(Sha256::digest(content.as_bytes()))),
                content,
            });
        }
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

        assert!(
            document.chunks[0]
                .content
                .starts_with("## Current state\n\ncurrent target")
        );
        assert!(document.chunks[0].content.contains(&long_line));
    }
}
