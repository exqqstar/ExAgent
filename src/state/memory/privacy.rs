use super::types::MemoryPrivacyFlags;

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedMemoryText {
    pub text: String,
    pub flags: MemoryPrivacyFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPathSensitivity {
    Normal,
    Sensitive,
}

pub fn redact_memory_text(input: &str, max_chars: usize) -> RedactedMemoryText {
    let mut flags = MemoryPrivacyFlags::default();

    let text = redact_private_blocks(input, &mut flags);
    let mut text = redact_secret_lines(&text, &mut flags);

    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect();
        text.push_str("\n[TRUNCATED]");
        flags.output_truncated = true;
    }

    RedactedMemoryText { text, flags }
}

pub fn classify_memory_path(path: &str) -> MemoryPathSensitivity {
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches('/');
    let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);

    if basename == ".env"
        || basename.starts_with(".env.")
        || basename == ".ssh"
        || basename == "credentials"
        || basename.starts_with("credentials.")
        || matches!(basename, "id_rsa" | "id_ed25519")
        || lower.contains("/.ssh/")
        || lower.contains("secret")
        || lower.contains("secrets/")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
    {
        MemoryPathSensitivity::Sensitive
    } else {
        MemoryPathSensitivity::Normal
    }
}

fn redact_private_blocks(input: &str, flags: &mut MemoryPrivacyFlags) -> String {
    static PRIVATE_BLOCK_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        PRIVATE_BLOCK_RE.get_or_init(|| Regex::new(r"(?is)<private>.*?(?:</private>|$)").unwrap());

    if re.is_match(input) {
        flags.redacted_private_block = true;
        re.replace_all(input, "[REDACTED_PRIVATE_BLOCK]")
            .into_owned()
    } else {
        input.to_string()
    }
}

fn redact_secret_lines(input: &str, flags: &mut MemoryPrivacyFlags) -> String {
    let mut output = String::with_capacity(input.len());

    for segment in input.split_inclusive('\n') {
        let (line, ending) = split_line_ending(segment);
        if looks_like_secret_line(line) {
            flags.redacted_secret = true;
            output.push_str("[REDACTED_SECRET]");
            output.push_str(ending);
        } else {
            output.push_str(segment);
        }
    }

    output
}

fn split_line_ending(segment: &str) -> (&str, &str) {
    if let Some(line) = segment.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = segment.strip_suffix('\n') {
        (line, "\n")
    } else {
        (segment, "")
    }
}

fn looks_like_secret_line(line: &str) -> bool {
    static SECRET_LINE_RE: OnceLock<Regex> = OnceLock::new();
    let re = SECRET_LINE_RE.get_or_init(|| {
        Regex::new(
            r"(?i)(api[_-]?key|apikey|token\s*=|password\s*=|authorization\s*:\s*bearer|aws_secret_access_key|secret[_-]?key|private[_-]?key|client[_-]?secret|-----begin [a-z0-9 ]*private key-----|sk-[A-Za-z0-9_-]+|ghp_[A-Za-z0-9_]+)",
        )
        .unwrap()
    });

    re.is_match(line)
}
