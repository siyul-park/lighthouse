use lighthouse_model::SignalClass::{self, Abstain, Correction, Directive, None, Preference};
use lighthouse_session::signal::{rules, said};

fn class(prompt: &str) -> (SignalClass, &'static str) {
    let said = said(prompt);
    (said.class, said.rule)
}

#[test]
fn every_rule_fires_on_a_sentence_written_for_it() {
    let cases = [
        (
            "en-no-open",
            Correction,
            "No, put the error type in its own module.",
        ),
        ("en-not-what", Correction, "That's not what I asked for."),
        (
            "en-i-said",
            Correction,
            "I said use the helper, not copy it.",
        ),
        (
            "en-instead",
            Correction,
            "Use a trait object instead of generics.",
        ),
        ("en-actually", Correction, "Actually, keep it synchronous."),
        (
            "en-dont-do-that",
            Correction,
            "Don't do that, it breaks the API.",
        ),
        (
            "en-not-x-but-y",
            Correction,
            "Use not_found, but only here. Not foo, use bar.",
        ),
        ("ko-no-open", Correction, "아니, 별도 모듈에 둬야지."),
        ("ko-not-that", Correction, "그게 아니라 기존 헬퍼를 쓰라고."),
        ("ko-instead", Correction, "HashMap 말고 BTreeMap을 써."),
        ("ko-wrong", Correction, "그 방식은 틀렸어."),
        (
            "en-always-never",
            Directive,
            "Never use unwrap in library code.",
        ),
        (
            "en-dont-verb",
            Directive,
            "Please don't add new dependencies.",
        ),
        ("en-must", Directive, "A handler must return a typed error."),
        (
            "en-should-always",
            Directive,
            "Callers should only pass owned buffers.",
        ),
        (
            "en-all-must",
            Directive,
            "Every endpoint needs to have a test.",
        ),
        (
            "en-from-now-on",
            Directive,
            "From now on, name flags in kebab-case.",
        ),
        (
            "en-we-convention",
            Directive,
            "We only talk to the cache through the port.",
        ),
        (
            "en-convention",
            Directive,
            "The rule is one module per file.",
        ),
        ("en-only-use", Directive, "Only use tracing for logging."),
        ("en-follow", Directive, "Follow the repo's conventions."),
        ("ko-always-never", Directive, "에러는 항상 Result로 반환해."),
        ("ko-dont", Directive, "시크릿을 설정 파일에 넣지 마세요."),
        ("ko-must", Directive, "커밋 메시지는 영어로 작성할 것."),
        ("ko-from-now-on", Directive, "앞으로 문서 주석을 달아줘."),
        (
            "ko-we",
            Directive,
            "우리 프로젝트에서는 모듈당 파일 하나만 둔다.",
        ),
        ("ko-only", Directive, "로깅은 tracing으로만 써."),
        (
            "ko-convention",
            Directive,
            "규칙은 이렇다. 파일 하나에 모듈 하나.",
        ),
        (
            "en-prefer",
            Preference,
            "I prefer early returns over nested ifs.",
        ),
        (
            "en-is-better",
            Preference,
            "provider.Provider reads better than the long name.",
        ),
        (
            "en-better-than",
            Preference,
            "A plain struct, cleaner than a builder.",
        ),
        (
            "en-rather",
            Preference,
            "I'd rather have two small functions.",
        ),
        (
            "en-like-style",
            Preference,
            "I like the flat layout of the modules.",
        ),
        (
            "en-i-think-better",
            Preference,
            "I think the iterator is the cleaner option.",
        ),
        ("ko-more-good", Preference, "여기선 struct가 더 깔끔해."),
        ("ko-is-better", Preference, "컴포지션이 상속보다 낫다고 봐."),
        ("ko-prefer", Preference, "CLI 플래그는 kebab-case를 선호해."),
        ("en-never-mind", None, "Never mind."),
        ("ko-ack", None, "알겠습니다."),
    ];
    let table: Vec<_> = rules().collect();
    for (id, expected, prompt) in cases {
        let (got, rule) = class(prompt);
        assert_eq!((got, rule), (expected, id), "{prompt}");
        assert!(table.contains(&(id, expected)), "{id}");
    }
    for (id, _) in &table {
        assert!(
            cases.iter().any(|(case, _, _)| case == id),
            "rule `{id}` has no case"
        );
    }
}

#[test]
fn corrections_win_over_directives_and_directives_over_preferences() {
    assert_eq!(class("No, never use unwrap.").0, Correction);
    assert_eq!(class("I prefer it, but always add a test.").0, Directive);
}

#[test]
fn a_message_no_rule_decides_abstains() {
    for prompt in [
        "Add a --verbose flag to the check command.",
        "check 명령에 --verbose 플래그를 추가해줘.",
        "",
    ] {
        assert_eq!(class(prompt), (Abstain, ""), "{prompt}");
    }
}

#[test]
fn a_question_is_never_a_decision() {
    for prompt in [
        "Should we always run clippy in CI?",
        "Why did you never use the helper?",
        "어떤 걸 항상 써야 해?",
        "이 함수를 절대 호출하면 안 되나요?",
    ] {
        assert_eq!(class(prompt).0, Abstain, "{prompt}");
    }
}

#[test]
fn words_of_a_feature_or_a_path_are_not_a_decision() {
    for prompt in [
        "Add an always-on-top option to the window settings.",
        "절대경로로 바꿔줘.",
        "Never mind, I found it myself. Thanks.",
    ] {
        let said = said(prompt);
        assert!(
            !matches!(said.class, Directive | Correction | Preference),
            "{prompt}: {:?}",
            said.class
        );
    }
}

#[test]
fn a_decision_inside_quoted_or_logged_text_is_not_the_persons() {
    let quoted = "What does this mean?\n> You should always use tabs.\n> Never use spaces.";
    let fenced = "Fix the build:\n```\n// never use unsafe here\nlet x = 1;\n```";
    let log = "It fails:\n2026-10-29 10:00:00 ERROR you must not call this twice";
    for prompt in [quoted, fenced, log] {
        assert_eq!(class(prompt).0, Abstain, "{prompt}");
    }
    let mixed = format!("{fenced}\nAnd from now on, name flags in kebab-case.");
    assert_eq!(class(&mixed), (Directive, "en-from-now-on"));
}

#[test]
fn the_excerpt_is_the_sentence_cut_to_200_characters_with_secrets_hidden() {
    let long = format!("Always use {}.", "x".repeat(400));
    let cut = said(&long);
    assert_eq!(cut.excerpt.chars().count(), 200);
    assert!(cut.excerpt.ends_with('…'));

    let keyed = said("Never paste sk-ant-abcdefghijklmnopqrstuvwxyz0123456789 into a config.");
    assert!(!keyed.excerpt.contains("sk-ant"), "{}", keyed.excerpt);
    assert!(keyed.excerpt.contains("[REDACTED]"));
}

#[test]
fn features_say_the_language_and_size_but_not_the_text() {
    let said = said("에러는 항상 Result로 반환해야 해.\n```\nlet x = 1;\n```");
    assert_eq!(said.features["language"], "ko");
    assert_eq!(said.features["hadCode"], true);
    assert_eq!(said.features["hadPasted"], false);
    assert!(!said.features.to_string().contains("Result"));
}
