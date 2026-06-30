//! Article chat: RAG pipeline, persistence orchestration, and the agentic
//! web-search loop.
//!
//! This module currently provides paragraph chunking for RAG; the retrieval
//! pipeline and agentic loop are added in a subsequent step.

/// Merge paragraphs shorter than this (in words) with following ones.
const MIN_CHUNK_WORDS: usize = 40;
/// Never let a chunk exceed this many words; oversized paragraphs are windowed.
const MAX_CHUNK_WORDS: usize = 200;

/// Splits article text into retrieval chunks.
///
/// Paragraphs are detected by blank-line boundaries (linear scan, no regex),
/// then greedily merged so each chunk is roughly [`MIN_CHUNK_WORDS`]–
/// [`MAX_CHUNK_WORDS`] words: tiny paragraphs are combined, and any single
/// paragraph longer than the max is split into fixed word windows. Empty input
/// yields no chunks (the caller then falls back to full context).
pub fn chunk_paragraphs(text: &str) -> Vec<String> {
    let paragraphs = split_paragraphs(text);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_words = 0usize;

    for paragraph in paragraphs {
        let words = paragraph.split_whitespace().count();
        if words == 0 {
            continue;
        }

        // An oversized paragraph can't be merged; flush, then window it.
        if words > MAX_CHUNK_WORDS {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_words = 0;
            }
            chunks.extend(split_into_word_windows(&paragraph, MAX_CHUNK_WORDS));
            continue;
        }

        // Flush before exceeding the max when we already have content.
        if !current.is_empty() && current_words + words > MAX_CHUNK_WORDS {
            chunks.push(std::mem::take(&mut current));
            current_words = 0;
        }

        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(&paragraph);
        current_words += words;

        // Flush once the chunk is large enough to be useful on its own.
        if current_words >= MIN_CHUNK_WORDS {
            chunks.push(std::mem::take(&mut current));
            current_words = 0;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Groups runs of non-blank lines into paragraphs, splitting on blank lines.
/// Lines within a paragraph are joined with a single space and trimmed.
fn split_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line.trim());
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Splits a long paragraph into chunks of at most `max_words` words.
fn split_into_word_windows(paragraph: &str, max_words: usize) -> Vec<String> {
    paragraph
        .split_whitespace()
        .collect::<Vec<_>>()
        .chunks(max_words)
        .map(|window| window.join(" "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_or_blank_text_yields_no_chunks() {
        assert!(chunk_paragraphs("").is_empty());
        assert!(chunk_paragraphs("   \n\n  \n\t\n").is_empty());
    }

    #[test]
    fn test_small_paragraphs_are_merged() {
        // Six 10-word paragraphs (60 words). Merging to >= 40 words yields one
        // flushed chunk of ~40 words and a trailing chunk with the remainder.
        let ten = "one two three four five six seven eight nine ten";
        let text = [ten; 6].join("\n\n");
        let chunks = chunk_paragraphs(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].split_whitespace().count(), 40);
        assert_eq!(chunks[1].split_whitespace().count(), 20);
        // Merged paragraphs are separated by a blank line.
        assert!(chunks[0].contains("\n\n"));
    }

    #[test]
    fn test_oversized_paragraph_is_windowed() {
        let big = (0..450)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_paragraphs(&big);
        // 450 words → 200 + 200 + 50.
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].split_whitespace().count(), MAX_CHUNK_WORDS);
        assert_eq!(chunks[1].split_whitespace().count(), MAX_CHUNK_WORDS);
        assert_eq!(chunks[2].split_whitespace().count(), 50);
    }

    #[test]
    fn test_paragraph_boundaries_respect_blank_lines_not_single_newlines() {
        // Single newlines are part of one paragraph; a blank line separates them.
        let text = "line one\nline two\n\nsecond paragraph";
        let paras = split_paragraphs(text);
        assert_eq!(paras, vec!["line one line two", "second paragraph"]);
    }
}
