use super::text::{fingerprint, redact};
use super::{Detail, Grouper, LogLine, Stream};
use jiff::Timestamp;
use skym_core::model::{ExceptionClass, ExceptionGroup};
use skym_core::subject::WorkloadKey;

fn key() -> WorkloadKey {
    WorkloadKey { host: "x".into(), project: "captain".into(), service: "api".into() }
}

fn ts(sec: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + sec).unwrap()
}

fn group(lines: &[(Stream, &str)], detail: Detail) -> Vec<ExceptionGroup> {
    let mut g = Grouper::new(key(), detail);
    for (i, (stream, text)) in lines.iter().enumerate() {
        g.push(LogLine { ts: ts(i as i64), stream: *stream, text });
    }
    g.finish()
}

fn codes(groups: &[ExceptionGroup]) -> Vec<(&str, &str, u32)> {
    groups.iter().map(|g| (g.component.as_str(), g.code.as_str(), g.count)).collect()
}

const OUT: Stream = Stream::Stdout;
const ERR: Stream = Stream::Stderr;

#[test]
fn marked_lines_group_by_class_component_code() {
    let line = |biz: &str, fin: &str| {
        format!(
            r#"{{"skym":"exception","class":"application","component":"structurize","code":"LLM_TIMEOUT","msg":"timeout","biz_key":"{biz}"{fin}}}"#
        )
    };
    let lines: Vec<String> = (1..=7)
        .map(|i| line(&format!("notice:{}", i % 6), if i == 1 { r#","final":false"# } else { "" }))
        .collect();
    let input: Vec<(Stream, &str)> = lines.iter().map(|l| (OUT, l.as_str())).collect();
    let groups = group(&input, Detail::Report);
    assert_eq!(codes(&groups), [("structurize", "LLM_TIMEOUT", 7)]);
    let g = &groups[0];
    assert_eq!(g.final_count, 6, "final defaults to true");
    assert_eq!((g.first_seen, g.last_seen), (ts(0), ts(6)));
    assert_eq!(g.biz_keys, ["notice:3", "notice:4", "notice:5", "notice:0", "notice:1"]);
    assert_eq!(g.sample.as_ref().unwrap().message, "timeout");
}

#[test]
fn malformed_marked_lines_are_protocol_errors_and_unknown_kinds_are_ignored() {
    let bad = [
        r#"{"skym":"exception","class":"oops","component":"a","code":"X"}"#,
        r#"{"skym":"exception","class":"application","component":"Bad Name","code":"X"}"#,
        r#"{"skym":"exception","class":"application","component":"a","code":"lower"}"#,
        r#"{"skym":"exception","class":"application","component":"a"}"#,
        r#"{"skym":"exception","class":"application","component":"a","code":"X","final":"yes"}"#,
        r#"{"skym": "exception", broken"#,
    ];
    let mut input: Vec<(Stream, &str)> = bad.iter().map(|l| (OUT, *l)).collect();
    input.push((OUT, r#"{"skym":"event","anything":1}"#));
    input.push((OUT, r#"{"level":"error","msg":"no marker on stdout"}"#));
    assert_eq!(codes(&group(&input, Detail::Report)), [("_skym", "_PROTOCOL_ERROR", 6)]);
}

#[test]
fn a_protocol_error_says_what_is_wrong_without_echoing_the_line() {
    let cases = [
        (r#"{"skym": "exception", broken"#, "not valid JSON"),
        (r#"{"skym":1,"class":"application"}"#, "wrong field type"),
        (
            r#"{"skym":"exception","class":"x","class":"application","component":"a","code":"X"}"#,
            "wrong field type or repeated field",
        ),
        (r#"{"skym":"exception","component":"a","code":"X"}"#, "class must be"),
        (
            r#"{"skym":"exception","class":"business","component":"Bad Name","code":"X"}"#,
            "component must",
        ),
        (
            r#"{"skym":"exception","class":"application","component":"a","code":"secret-9"}"#,
            "code must",
        ),
    ];
    for (line, reason) in cases {
        let groups = group(&[(OUT, line)], Detail::Local);
        let message = &groups[0].sample.as_ref().unwrap().message;
        assert!(message.starts_with(reason), "{line}: {message}");
        assert!(!message.contains("secret") && !message.contains("Bad Name"), "{message}");
    }
}

#[test]
fn stderr_fallback_keeps_error_like_records_with_their_stack() {
    let input = [
        (ERR, "INFO 2026-10-01 worker started, 0 errors so far"),
        (ERR, "TypeError: Cannot read properties of undefined (reading 'id')"),
        (ERR, "    at handler (/app/src/api.js:12:5)"),
        (OUT, "GET /health 200"),
        (ERR, "    at next (/app/node_modules/express/lib/router.js:1:1)"),
        (ERR, "listening on 4001"),
        (ERR, "TypeError: Cannot read properties of undefined (reading 'name')"),
    ];
    let groups = group(&input, Detail::Report);
    assert_eq!(groups.len(), 1, "same fingerprint despite different quoted text");
    let g = &groups[0];
    assert_eq!((g.component.as_str(), g.count), ("_stderr", 2));
    assert!(g.code.starts_with("_UNSTRUCTURED:"));
    let first = group(&input[..5], Detail::Report);
    let stack = first[0].sample.as_ref().unwrap().stacktrace.as_deref().unwrap();
    assert_eq!(stack.lines().count(), 2);
}

#[test]
fn stderr_lines_with_a_lower_level_are_not_errors() {
    let quiet = [
        r#"time="2026-10-02T07:23:05Z" level=warning msg="cleanup" error="exit status 2""#,
        r#"{"level":"info","msg":"retrying after error"}"#,
        "2026-10-02 07:23:05,123 INFO [app.py:12] - task failed with error, will retry",
        "WARNING:root:connection error, retrying",
    ];
    let loud = [
        r#"time="2026-10-02T07:23:05Z" level=error msg="Error setting up exec""#,
        "Error fetching user info for id 42",
        "ValueError: invalid literal for int()",
        "thread 'main' panicked at src/main.rs:3:5",
    ];
    for line in quiet {
        assert!(group(&[(ERR, line)], Detail::Report).is_empty(), "{line}");
    }
    for line in loud {
        assert_eq!(group(&[(ERR, line)], Detail::Report).len(), 1, "{line}");
    }
}

#[test]
fn python_tracebacks_are_one_record_named_by_their_last_line() {
    let traceback = |error: &'static str| {
        [
            (ERR, "Traceback (most recent call last):"),
            (ERR, "  File \"/app/task.py\", line 12, in run"),
            (ERR, "    total = int(value)"),
            (ERR, error),
        ]
    };
    let mut input = traceback("ValueError: invalid literal for int() with base 10: 'x'").to_vec();
    input.extend(traceback("ValueError: invalid literal for int() with base 10: 'y'"));
    input.extend(traceback("KeyNotFound: 'user'"));
    let groups = group(&input, Detail::Report);
    let mut summary: Vec<(&str, u32)> = groups
        .iter()
        .map(|g| (g.sample.as_ref().unwrap().message.split(':').next().unwrap(), g.count))
        .collect();
    summary.sort();
    assert_eq!(summary, [("KeyNotFound", 1), ("ValueError", 2)]);
    let stack = groups[0].sample.as_ref().unwrap().stacktrace.as_deref().unwrap();
    assert_eq!(stack.lines().count(), 2, "the frames, without the header or the summary");
}

#[test]
fn logged_and_chained_python_exceptions_are_one_record() {
    let logged = [
        (ERR, "ERROR:root:import failed for batch 42"),
        (ERR, "Traceback (most recent call last):"),
        (ERR, "  File \"/app/import.py\", line 8, in run"),
        (ERR, "KeyError: 'sku'"),
    ];
    let chained = [
        (ERR, "Traceback (most recent call last):"),
        (ERR, "  File \"/app/db.py\", line 3, in get"),
        (ERR, "TimeoutError: pool exhausted"),
        (ERR, ""),
        (ERR, "During handling of the above exception, another exception occurred:"),
        (ERR, ""),
        (ERR, "Traceback (most recent call last):"),
        (ERR, "  File \"/app/api.py\", line 9, in handler"),
        (ERR, "RuntimeError: request failed"),
        (ERR, "worker idle"),
    ];
    let one = group(&logged, Detail::Report);
    assert_eq!(one.len(), 1);
    let sample = one[0].sample.as_ref().unwrap();
    assert_eq!(
        (one[0].count, sample.message.as_str()),
        (1, "ERROR:root:import failed for batch 42")
    );
    assert!(sample.stacktrace.as_deref().unwrap().ends_with("KeyError: 'sku'"));
    let both = group(&chained, Detail::Report);
    assert_eq!(both.len(), 1, "a chain is one failure");
    let sample = both[0].sample.as_ref().unwrap();
    assert_eq!((both[0].count, sample.message.as_str()), (1, "RuntimeError: request failed"));
    assert!(sample.stacktrace.as_deref().unwrap().contains("During handling"));
}

#[test]
fn a_traceback_does_not_swallow_neighbouring_errors() {
    let input = [
        (ERR, "Error: upstream reset"),
        (ERR, "    at fetch (/app/client.js:4:1)"),
        (ERR, "Traceback (most recent call last):"),
        (ERR, "  File \"/app/x.py\", line 1, in <module>"),
        (ERR, "ValueError: bad"),
        (ERR, "ConnectionError: refused"),
    ];
    let mut messages: Vec<String> =
        group(&input, Detail::Report).into_iter().map(|g| g.sample.unwrap().message).collect();
    messages.sort();
    assert_eq!(messages, ["ConnectionError: refused", "Error: upstream reset", "ValueError: bad"]);
}

#[test]
fn same_code_in_both_classes_stays_apart() {
    let line = |class: &str| {
        format!(r#"{{"skym":"exception","class":"{class}","component":"c","code":"X"}}"#)
    };
    let (app, biz) = (line("application"), line("business"));
    let groups = group(&[(OUT, &app), (OUT, &biz), (OUT, &app)], Detail::Report);
    let counts: Vec<(ExceptionClass, u32)> = groups.iter().map(|g| (g.class, g.count)).collect();
    assert_eq!(counts, [(ExceptionClass::Application, 2), (ExceptionClass::Business, 1)]);
}

#[test]
fn only_json_lines_can_carry_the_marker() {
    let line = r#"Error: config key "skym" is missing"#;
    let groups = group(&[(ERR, line)], Detail::Report);
    assert_eq!(groups[0].component, "_stderr", "plain text mentioning skym is not a protocol line");
}

#[test]
fn groups_beyond_the_limit_fold_into_overflow() {
    let lines: Vec<String> = (0..105)
        .map(|i| {
            format!(r#"{{"skym":"exception","class":"application","component":"c","code":"E{i}"}}"#)
        })
        .collect();
    let input: Vec<(Stream, &str)> = lines.iter().map(|l| (OUT, l.as_str())).collect();
    let groups = group(&input, Detail::Report);
    assert_eq!(groups.len(), 101);
    let overflow = groups.iter().find(|g| g.code == "_OVERFLOW").unwrap();
    assert_eq!((overflow.component.as_str(), overflow.count), ("_skym", 5));
}

#[test]
fn report_detail_bounds_samples_and_drops_business_ones() {
    let long = "x".repeat(1500);
    let app = format!(
        r#"{{"skym":"exception","class":"application","component":"a","code":"A","message":"{long}"}}"#
    );
    let biz = r#"{"skym":"exception","class":"business","component":"b","code":"B","message":"m"}"#;
    let input = [(OUT, app.as_str()), (OUT, biz)];
    let sample = |groups: &[ExceptionGroup], class| {
        groups.iter().find(|g| g.class == class).unwrap().sample.clone()
    };
    let report = group(&input, Detail::Report);
    assert_eq!(sample(&report, ExceptionClass::Application).unwrap().message.chars().count(), 1000);
    assert_eq!(sample(&report, ExceptionClass::Business), None);
    let local = group(&input, Detail::Local);
    assert_eq!(sample(&local, ExceptionClass::Application).unwrap().message.len(), 1500);
    assert!(sample(&local, ExceptionClass::Business).is_some());
}

#[test]
fn fingerprints_ignore_variable_parts() {
    let same = [
        "user 42 not found (id 3f2a9c1e-1b2c-4d5e-8f90-123456789abc) at 0x7ffe12",
        "user 7 not found (id 00000000-0000-0000-0000-000000000000) at 0xdead",
    ];
    assert_eq!(fingerprint(same[0]), fingerprint(same[1]));
    assert_ne!(fingerprint("user not found"), fingerprint("order not found"));
    assert_eq!(fingerprint("x").len(), 8);
}

#[test]
fn redaction_hides_common_secrets() {
    let cases = [
        ("Authorization: Bearer abc.def", "abc.def"),
        ("password=hunter2 user=bob", "hunter2"),
        ("DB_PASSWORD=hunter2", "hunter2"),
        ("PGPASSWORD=hunter2 psql", "hunter2"),
        ("client_secret=c-456&grant=x", "c-456"),
        ("AWS_SECRET_ACCESS_KEY=AKIAxyz", "AKIAxyz"),
        ("X_AUTH_TOKEN: t-789", "t-789"),
        (r#"{"dbPassword": "p-000"}"#, "p-000"),
        ("SECRET_KEY=django-insecure-abc", "django-insecure-abc"),
        (r#"password="correct horse battery""#, "horse"),
        ("Authorization: Basic dXNlcjpwYXNz", "dXNlcjpwYXNz"),
        (r#"{"api_key": "k-123"}"#, "k-123"),
        ("postgres://app:s3cret@db:5432/app", "s3cret"),
        ("token eyJhbGciOi.eyJzdWIiOi.c2lnbmF0dXJl here", "eyJzdWIiOi"),
        ("-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----", "MIIE"),
    ];
    for (input, secret) in cases {
        let out = redact(input);
        assert!(!out.contains(secret), "{input} → {out}");
        assert!(out.contains("[REDACTED]"), "{out}");
    }
    assert_eq!(redact("nothing secret here"), "nothing secret here");
}
