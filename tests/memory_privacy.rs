use exagent::state::memory::privacy::{
    classify_memory_path, redact_memory_text, MemoryPathSensitivity,
};

#[test]
fn redacts_common_secret_shapes() {
    let fake_openai_key = ["OPENAI_API_KEY=", "sk-proj-abc123"].concat();
    let fake_github_token = ["GITHUB_TOKEN=", "ghp_", "placeholder"].concat();
    let fake_sk_literal = ["literal ", "sk-test-secret"].concat();
    let fake_private_key_header = format!("-----BEGIN {} KEY-----", "PRIVATE");
    let input = [
        "normal context survives",
        fake_openai_key.as_str(),
        fake_github_token.as_str(),
        "service apikey: secret-value",
        "password=hunter2",
        "authorization: bearer abc123",
        fake_sk_literal.as_str(),
        "AWS_SECRET_ACCESS_KEY=aws-secret",
        "SECRET_KEY=django-secret",
        "PRIVATE_KEY=private-secret",
        "client_secret=oauth-secret",
        fake_private_key_header.as_str(),
    ]
    .join("\n");

    let redacted = redact_memory_text(&input, 2048);

    assert!(redacted.flags.redacted_secret);
    assert!(redacted.text.contains("normal context survives"));
    assert!(!redacted.text.contains(&fake_openai_key));
    assert!(!redacted.text.contains(&fake_github_token));
    assert!(!redacted.text.contains("hunter2"));
    assert!(!redacted.text.contains("authorization: bearer abc123"));
    assert!(!redacted.text.contains(&fake_sk_literal));
    assert!(!redacted.text.contains("aws-secret"));
    assert!(!redacted.text.contains("django-secret"));
    assert!(!redacted.text.contains("private-secret"));
    assert!(!redacted.text.contains("oauth-secret"));
    assert!(!redacted.text.contains(&fake_private_key_header));
    assert_eq!(
        redacted
            .text
            .lines()
            .filter(|line| *line == "[REDACTED_SECRET]")
            .count(),
        11
    );
}

#[test]
fn redacts_private_blocks() {
    let redacted = redact_memory_text("safe <private>secret</private> safe", 2048);

    assert_eq!(redacted.text, "safe [REDACTED_PRIVATE_BLOCK] safe");
    assert!(redacted.flags.redacted_private_block);
    assert!(!redacted.text.contains("secret"));
}

#[test]
fn redacts_unclosed_private_blocks_to_eof() {
    let redacted = redact_memory_text("safe <private>internal note", 2048);

    assert_eq!(redacted.text, "safe [REDACTED_PRIVATE_BLOCK]");
    assert!(redacted.flags.redacted_private_block);
    assert!(!redacted.text.contains("internal note"));
}

#[test]
fn redacts_multiple_private_blocks() {
    let redacted = redact_memory_text("a <private>one</private> b <private>two</private> c", 2048);

    assert_eq!(
        redacted.text,
        "a [REDACTED_PRIVATE_BLOCK] b [REDACTED_PRIVATE_BLOCK] c"
    );
    assert!(redacted.flags.redacted_private_block);
    assert!(!redacted.text.contains("one"));
    assert!(!redacted.text.contains("two"));
}

#[test]
fn marks_sensitive_paths() {
    for path in [
        ".env",
        ".env.staging",
        "/work/.env.local",
        "/work/.env.production",
        "~/.ssh",
        ".ssh",
        "/home/user/.ssh",
        "/home/user/.ssh/",
        "/home/user/.ssh/id_rsa",
        "/home/user/.ssh/id_ed25519",
        "config/credentials",
        "config/credentials.json",
        "config/secrets/api.json",
        "src/secret_config.rs",
        "src/my_secret.rs",
        "certs/client.pem",
        "keys/client.key",
    ] {
        assert_eq!(
            classify_memory_path(path),
            MemoryPathSensitivity::Sensitive,
            "{path} should be sensitive"
        );
    }

    assert_eq!(
        classify_memory_path("src/runtime/context.rs"),
        MemoryPathSensitivity::Normal
    );
}

#[test]
fn truncates_large_outputs_before_indexing() {
    let input = "abcdef".repeat(20);
    let redacted = redact_memory_text(&input, 17);

    assert!(redacted.flags.output_truncated);
    assert_eq!(redacted.text, "abcdefabcdefabcde\n[TRUNCATED]");
}
