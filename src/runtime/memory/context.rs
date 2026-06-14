use crate::state::memory::{MemorySearchHit, MemorySourceKind};

const MIN_AUTO_INJECT_CONFIDENCE: f64 = 0.72;

pub fn format_auto_memory_context(hits: &[MemorySearchHit], max_chars: usize) -> String {
    format_memory_context("Relevant project memory:", hits, max_chars)
}

pub fn format_frozen_memory_block(hits: &[MemorySearchHit], max_chars: usize) -> String {
    format_memory_context("Pinned project memory:", hits, max_chars)
}

fn format_memory_context(header: &str, hits: &[MemorySearchHit], max_chars: usize) -> String {
    let header_chars = char_count(header) + 1;
    if max_chars <= header_chars {
        return String::new();
    }

    let mut rendered = String::with_capacity(max_chars.min(4096));
    rendered.push_str(header);
    rendered.push('\n');
    let mut rendered_chars = header_chars;
    let mut rendered_any = false;

    for hit in hits {
        if !strictly_injectable(hit) {
            continue;
        }

        let line = format_hit(hit);
        let line_chars = char_count(&line);
        let separator_chars = usize::from(rendered_any);
        if rendered_chars + separator_chars + line_chars > max_chars {
            continue;
        }
        if rendered_any {
            rendered.push('\n');
            rendered_chars += 1;
        }
        rendered.push_str(&line);
        rendered_chars += line_chars;
        rendered_any = true;
    }

    if rendered_any {
        rendered.trim_end().to_string()
    } else {
        String::new()
    }
}

fn strictly_injectable(hit: &MemorySearchHit) -> bool {
    if hit.quarantined
        || hit.stale
        || !hit.confidence.is_finite()
        || hit.confidence < MIN_AUTO_INJECT_CONFIDENCE
    {
        return false;
    }
    match hit.source {
        MemorySourceKind::Entry => hit.kind != "candidate",
        MemorySourceKind::Observation => hit.kind == "user_rule" && hit.auto_inject_eligible,
    }
}

fn format_hit(hit: &MemorySearchHit) -> String {
    let mut line = String::new();
    line.push_str("- [");
    line.push_str(hit.source.as_str());
    line.push(':');
    line.push_str(&hit.kind);
    line.push_str(" confidence=");
    line.push_str(&format!("{:.2}", hit.confidence));
    line.push_str("] ");
    line.push_str(&single_line(hit.title.trim()));

    let body = hit.body.trim();
    if !body.is_empty() {
        line.push('\n');
        line.push_str("  body: ");
        line.push_str(&single_line(body));
    }

    if !hit.files.is_empty() {
        line.push('\n');
        line.push_str("  files: ");
        line.push_str(&hit.files.join(", "));
    }

    line
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}
