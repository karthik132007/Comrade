/**
 * Sentence chunker: LLM token stream → speakable chunks.
 * Short fragments merge forward so TTS starts on ("Hey! I checked your
 * project.") instead of ("Hey!"). Pure + tested.
 */
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "st", "sr", "jr", "vs", "etc", "e.g", "i.e", "a.m", "p.m",
];

fn is_abbrev_dot(text: &str, dot_pos: usize) -> bool {
    let before: String = text[..dot_pos].chars().rev().take_while(|c| c.is_alphabetic()).collect();
    let word: String = before.chars().rev().collect::<String>().to_lowercase();
    ABBREVIATIONS.contains(&word.as_str())
}

/// Split text into speakable chunks. `max_chars` hard-caps; fragments shorter
/// than `min_merge` merge into the following chunk.
pub fn split_sentences(text: &str, max_chars: usize, min_merge: usize) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    // Newlines are always chunk boundaries (lists, paragraphs).
    for paragraph in text.split('\n') {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        let chars: Vec<char> = paragraph.chars().collect();
        // Precompute byte offsets for abbreviation checks.
        let byte_offsets: Vec<usize> = paragraph.char_indices().map(|(b, _)| b).collect();
        let mut start = 0usize;
        let mut i = 0usize;
        while i < chars.len() {
            let boundary = if matches!(chars[i], '.' | '!' | '?' | '…' | '。' | '！' | '？') {
                let mut is_end = {
                    let mut j = i + 1;
                    while j < chars.len() && matches!(chars[j], '"' | '\'' | '”' | '’' | ')') {
                        j += 1;
                    }
                    j >= chars.len() || chars[j].is_whitespace()
                };
                if chars[i] == '.' && is_end && is_abbrev_dot(paragraph, byte_offsets[i]) {
                    is_end = false;
                }
                is_end
            } else {
                false
            };
            if boundary {
                let mut end = i + 1;
                while end < chars.len() && matches!(chars[end], '"' | '\'' | '”' | '’') {
                    end += 1;
                }
                let piece: String = chars[start..end].iter().collect();
                if !piece.trim().is_empty() {
                    raw.push(piece.trim().to_string());
                }
                // Skip following whitespace.
                while end < chars.len() && chars[end].is_whitespace() {
                    end += 1;
                }
                start = end;
                i = end;
                continue;
            }
            i += 1;
        }
        if start < chars.len() {
            let tail: String = chars[start..].iter().collect();
            if !tail.trim().is_empty() {
                raw.push(tail.trim().to_string());
            }
        }
    }

    // Hard-split oversized pieces on word boundaries.
    let mut sized: Vec<String> = Vec::new();
    for piece in raw {
        if piece.chars().count() <= max_chars {
            sized.push(piece);
            continue;
        }
        let words: Vec<&str> = piece.split_whitespace().collect();
        let mut current = String::new();
        for word in words {
            let extra = if current.is_empty() { 0 } else { 1 } + word.chars().count();
            if current.chars().count() + extra > max_chars && !current.is_empty() {
                sized.push(current.trim().to_string());
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        if !current.trim().is_empty() {
            sized.push(current.trim().to_string());
        }
    }

    // Merge short fragments forward so TTS doesn't stutter on ("Hey!").
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sized.len() {
        let mut chunk = sized[i].clone();
        while chunk.chars().count() < min_merge && i + 1 < sized.len() {
            i += 1;
            chunk.push(' ');
            chunk.push_str(&sized[i]);
        }
        out.push(chunk);
        i += 1;
    }
    out.into_iter().filter(|c| !c.trim().is_empty()).collect()
}

/// Sentence segmentation with byte spans into the source.
/// Each item is (trimmed text, start byte, end byte). No size limits, no merging.
fn sentence_pieces(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    // char index -> byte offset lookup.
    let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let chars: Vec<char> = text.chars().collect();
    let byte_at = |ci: usize| -> usize {
        if ci < bytes.len() {
            bytes[ci]
        } else {
            text.len()
        }
    };
    let mut i = 0usize;
    let mut seg_start = 0usize;
    let mut started = false;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_whitespace() && !started {
            seg_start = i;
            started = true;
        }
        if c == '\n' {
            if started {
                push_span(text, &mut out, &byte_at, seg_start, i);
                started = false;
            }
            i += 1;
            continue;
        }
        if matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？') {
            let mut is_end = {
                let mut j = i + 1;
                while j < chars.len() && matches!(chars[j], '"' | '\'' | '”' | '’' | ')') {
                    j += 1;
                }
                j >= chars.len() || chars[j].is_whitespace()
            };
            if c == '.' && is_end && is_abbrev_dot(text, byte_at(i)) {
                is_end = false;
            }
            if is_end {
                let mut end = i + 1;
                while end < chars.len() && matches!(chars[end], '"' | '\'' | '”' | '’') {
                    end += 1;
                }
                push_span(text, &mut out, &byte_at, seg_start, end);
                started = false;
                while end < chars.len() && chars[end].is_whitespace() && chars[end] != '\n' {
                    end += 1;
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    if started {
        push_span(text, &mut out, &byte_at, seg_start, chars.len());
    }
    out
}

fn push_span(
    text: &str,
    out: &mut Vec<(String, usize, usize)>,
    byte_at: &dyn Fn(usize) -> usize,
    start_ci: usize,
    end_ci: usize,
) {
    let start_b = byte_at(start_ci);
    let end_b = byte_at(end_ci);
    if start_b >= end_b {
        return;
    }
    let raw = &text[start_b..end_b];
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let lead = raw.len() - raw.trim_start().len();
    let trail = raw.len() - raw.trim_end().len();
    out.push((trimmed.to_string(), start_b + lead, end_b - trail));
}

/// Word-split one oversized piece, preserving byte spans.
/// `piece` must be an exact slice `text[base..]`.
fn hard_split_span(text: &str, base: usize, piece: &str, max_chars: usize) -> Vec<(String, usize, usize)> {
    let mut words: Vec<(&str, usize, usize)> = Vec::new();
    let mut search_from = 0usize;
    for w in piece.split_whitespace() {
        if let Some(rel) = piece[search_from..].find(w) {
            let ws = search_from + rel;
            words.push((w, base + ws, base + ws + w.len()));
            search_from = ws + w.len();
        }
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_start = 0usize;
    let mut cur_end = 0usize;
    for (w, ws, we) in words {
        let extra = if cur.is_empty() { 0 } else { 1 } + w.chars().count();
        if cur.chars().count() + extra > max_chars && !cur.is_empty() {
            out.push((cur.trim().to_string(), cur_start, cur_end));
            cur = String::new();
        }
        if cur.is_empty() {
            cur_start = ws;
        } else {
            cur.push(' ');
        }
        cur.push_str(w);
        cur_end = we;
    }
    if !cur.trim().is_empty() {
        out.push((cur.trim().to_string(), cur_start, cur_end));
    }
    // `text` is only for debug parity; spans derive from piece+base.
    let _ = text;
    out
}

/// Incremental chunker over a token stream: feed tokens, drain ready sentences.
pub struct SentenceBuffer {
    max_chars: usize,
    min_merge: usize,
    pending: String,
}

impl SentenceBuffer {
    pub fn new(max_chars: usize, min_merge: usize) -> Self {
        Self { max_chars, min_merge, pending: String::new() }
    }

    pub fn push_token(&mut self, token: &str) -> Vec<String> {
        self.pending.push_str(token);
        self.drain_ready()
    }

    fn drain_ready(&mut self) -> Vec<String> {
        // Segment with byte spans; fuse short pieces forward; emit every
        // fused piece except a held tail. Draining is byte-exact, so the
        // remainder keeps its original spacing for the next tokens.
        let mut sized: Vec<(String, usize, usize)> = Vec::new();
        for (t, sb, eb) in sentence_pieces(&self.pending) {
            if t.chars().count() <= self.max_chars {
                sized.push((t, sb, eb));
            } else {
                sized.extend(hard_split_span(&self.pending, sb, &t, self.max_chars));
            }
        }
        if sized.is_empty() {
            return vec![];
        }
        struct Fused {
            text: String,
            end: usize,
        }
        let mut fused: Vec<Fused> = Vec::new();
        let mut short_text = String::new();
        let mut short_end = 0usize;
        for (t, _sb, eb) in sized {
            let (text, end) = if short_text.is_empty() {
                (t, eb)
            } else {
                short_text.push(' ');
                short_text.push_str(&t);
                (std::mem::take(&mut short_text), eb)
            };
            if text.chars().count() < self.min_merge {
                short_text = text;
                short_end = end;
            } else {
                fused.push(Fused { text, end });
            }
        }
        let _ = short_end;
        let emit_n = match fused.last() {
            Some(f) if ends_with_boundary(f.text.trim_end()) => fused.len(),
            _ => fused.len().saturating_sub(1),
        };
        let mut ready = Vec::with_capacity(emit_n);
        let mut drain_end = 0usize;
        for f in fused.drain(..emit_n) {
            drain_end = f.end;
            ready.push(f.text);
        }
        self.pending = self.pending[drain_end..].to_string();
        ready
    }

    /// Call at stream end; applies min-merge across the remainder.
    pub fn finish(mut self) -> Vec<String> {
        let chunks = split_sentences(&self.pending, self.max_chars, self.min_merge);
        self.pending.clear();
        chunks
    }
}

fn ends_with_boundary(s: &str) -> bool {
    let mut chars = s.chars().rev();
    // Skip closing quotes.
    let mut c = chars.next();
    while matches!(c, Some('"' | '\'' | '”' | '’' | ')')) {
        c = chars.next();
    }
    matches!(c, Some('.' | '!' | '?' | '…' | '。' | '！' | '？'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_merges_short_fragments() {
        let chunks = split_sentences(
            "Hey! I checked your project. There is one problem with the configuration.",
            200,
            12,
        );
        assert_eq!(
            chunks,
            vec![
                "Hey! I checked your project.".to_string(),
                "There is one problem with the configuration.".to_string(),
            ]
        );
    }

    #[test]
    fn respects_abbreviations_and_quotes() {
        let chunks = split_sentences("Meet Dr. Smith at 5 p.m. \"Are you sure?\" Yes!", 200, 0);
        assert_eq!(
            chunks,
            vec![
                "Meet Dr. Smith at 5 p.m.".to_string(),
                "\"Are you sure?\"".to_string(),
                "Yes!".to_string(),
            ]
        );
    }

    #[test]
    fn hard_splits_oversized_pieces() {
        let long = "word ".repeat(100);
        let chunks = split_sentences(&long, 50, 0);
        assert!(chunks.len() > 2);
        assert!(chunks.iter().all(|c| c.chars().count() <= 50));
    }

    #[test]
    fn incremental_buffer_emits_complete_sentences() {
        let mut buf = SentenceBuffer::new(200, 12);
        let mut out = Vec::new();
        for tok in ["Hey! ", "I checked ", "your project. ", "There is ", "one problem."] {
            out.extend(buf.push_token(tok));
        }
        // "Hey! I checked your project." completes mid-stream; the second
        // sentence completes on the final token.
        assert_eq!(
            out,
            vec![
                "Hey! I checked your project.".to_string(),
                "There is one problem.".to_string()
            ]
        );
        let rest = buf.finish();
        assert!(rest.is_empty());
    }

    #[test]
    fn buffer_holds_incomplete_tail() {
        let mut buf = SentenceBuffer::new(200, 0);
        assert!(buf.push_token("Partial thought without").is_empty());
        assert!(buf.push_token(" an ending").is_empty());
        assert_eq!(buf.finish(), vec!["Partial thought without an ending".to_string()]);
    }
}
