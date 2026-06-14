#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InjectionScan {
    pub suspicious: bool,
    pub reasons: Vec<&'static str>,
}

pub fn scan_injection(text: &str) -> InjectionScan {
    let lower = text.to_ascii_lowercase();
    let mut scan = InjectionScan::default();

    if has_instruction_override(&lower) {
        push_reason(&mut scan, "instruction_override");
    }

    if has_exfiltration_shape(&lower) {
        push_reason(&mut scan, "exfiltration");
    }

    scan
}

fn has_instruction_override(text: &str) -> bool {
    [
        "ignore previous instructions",
        "ignore all previous",
        "disregard previous",
        "disregard all previous",
        "bypass approval",
        "bypass the approval",
        "always approve",
        "auto-approve",
        "skip confirmation",
        "without asking for confirmation",
        "run without asking",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn has_exfiltration_shape(text: &str) -> bool {
    has_shell_exfiltration_shape(text) || (has_sensitive_target(text) && has_outbound_action(text))
}

fn has_sensitive_target(text: &str) -> bool {
    [
        ".env",
        "id_rsa",
        "id_ed25519",
        "~/.ssh",
        "/.ssh/",
        "credentials",
        "private key",
        ".pem",
        ".key",
    ]
    .iter()
    .any(|target| text.contains(target))
        || text.contains("secrets/")
        || contains_word_token(text, "secret")
        || contains_word_token(text, "secrets")
}

fn has_outbound_action(text: &str) -> bool {
    ["post http", "post to http", "fetch http"]
        .iter()
        .any(|phrase| text.contains(phrase))
        || ["send", "upload", "curl", "wget", "exfiltrate"]
            .iter()
            .any(|action| contains_word_token(text, action))
}

fn has_shell_exfiltration_shape(text: &str) -> bool {
    has_sensitive_target(text)
        && (text.contains("$(cat ")
            || text.contains("cat ~/.ssh")
            || text.contains("cat .env")
            || (text.contains("cat ") && text.contains("base64")))
}

fn contains_word_token(text: &str, needle: &str) -> bool {
    text.match_indices(needle)
        .any(|(start, _)| has_token_boundaries(text, start, start + needle.len()))
}

fn has_token_boundaries(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    before.map_or(true, |ch| !is_token_char(ch)) && after.map_or(true, |ch| !is_token_char(ch))
}

fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn push_reason(scan: &mut InjectionScan, reason: &'static str) {
    scan.suspicious = true;
    if !scan.reasons.contains(&reason) {
        scan.reasons.push(reason);
    }
}
