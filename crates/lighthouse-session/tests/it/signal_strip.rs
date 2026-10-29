use lighthouse_session::signal::stripped;

#[test]
fn code_pasted_content_quotes_logs_and_urls_are_cut() {
    let prompt = "Please look at this.\n\
        ```rust\nfn main() {}\n```\n\
        > quoted line\n\
        thread 'main' panicked at src/lib.rs:1\n\
        See https://example.com/a?b=c and `inline` code.";
    let s = stripped(prompt);
    assert_eq!(s.text, "Please look at this.\nSee   and   code.");
    assert!(s.had_code);
    assert!(s.had_pasted);
}

#[test]
fn an_unclosed_fence_cuts_to_the_end() {
    assert_eq!(stripped("Keep this.\n```\nnever\nmore").text, "Keep this.");
}

#[test]
fn plain_text_is_untouched() {
    let s = stripped("Always use provider.Provider.");
    assert_eq!(s.text, "Always use provider.Provider.");
    assert!(!s.had_code && !s.had_pasted);
}

#[test]
fn an_indented_block_is_code() {
    let s = stripped("Look:\n    let x = never();\nDone");
    assert_eq!(s.text, "Look:\nDone");
    assert!(s.had_code);
}
