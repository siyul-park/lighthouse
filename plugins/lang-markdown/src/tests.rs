use lighthouse_protocol::{Source, language::HostTree, pointer_value};
use serde_json::Value;

use crate::provider;

fn tree_of(text: &str) -> HostTree {
    provider::parse(text).expect("the document parses")
}

fn source(text: &str) -> Source {
    let tree = tree_of(text);
    let source = Source {
        path: "doc.md".to_owned(),
        ast: tree.ast,
        mapping: tree.mapping,
    };
    source.validate(text).expect("the map is valid");
    source
}

/// The code blocks of the tree, in document order, as their pointers.
fn blocks(source: &Source) -> Vec<String> {
    let mut found = Vec::new();
    collect(&source.ast, "", &mut found);
    found
}

fn collect(value: &Value, pointer: &str, found: &mut Vec<String>) {
    if value["kind"] == "code_block" {
        found.push(pointer.to_owned());
    }
    for (index, child) in value["children"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        collect(child, &format!("{pointer}/children/{index}"), found);
    }
}

/// The host bytes a block's text maps to, each text segment in turn.
fn mapped(source: &Source, text: &str, block: &str) -> Vec<(String, String)> {
    let body = format!("{block}/text");
    let entry = &source.mapping[&body];
    let code = pointer_value(&source.ast, &body)
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();
    entry
        .text_map
        .as_ref()
        .expect("the body maps exactly")
        .iter()
        .map(|segment| {
            let [a, b] = segment.text_range.map(|at| at as usize);
            let [c, d] = segment.source_range.map(|at| at as usize);
            (code[a..b].to_owned(), text[c..d].to_owned())
        })
        .collect()
}

#[test]
fn a_fence_under_a_list_maps_through_the_indentation() {
    let text = "- item\n\n  ```rust\n  fn a() {}\n   x\n  ```\n";
    let source = source(text);
    let blocks = blocks(&source);
    assert_eq!(blocks.len(), 1);
    let block = &blocks[0];
    assert_eq!(
        pointer_value(&source.ast, &format!("{block}/text")),
        Some(&Value::String("fn a() {}\n x\n".to_owned()))
    );
    assert_eq!(
        mapped(&source, text, block),
        [
            ("fn a() {}\n".to_owned(), "fn a() {}\n".to_owned()),
            (" x\n".to_owned(), " x\n".to_owned()),
        ]
    );
    let body = &source.mapping[&format!("{block}/text")];
    let [start, _] = body.range.unwrap();
    assert_eq!(&text[start as usize..start as usize + 2], "fn");
}

#[test]
fn a_fence_in_a_blockquote_maps_past_the_markers() {
    let text = "> ```go\n> package a // é\n>\n> var x = 1\n> ```\n";
    let source = source(text);
    let block = &blocks(&source)[0];
    let segments = mapped(&source, text, block);
    assert_eq!(
        segments[0],
        ("package a // é\n".to_owned(), "package a // é\n".to_owned())
    );
    assert_eq!(segments[1].0, "\n");
    assert_eq!(segments[1].1, "\n");
    assert_eq!(segments[2].0, "var x = 1\n");
}

#[test]
fn tilde_and_backtick_fences_both_carry_their_text() {
    let text = "~~~yml\na: 1\n~~~\n\n```toml\nb = 2\n```\n";
    let source = source(text);
    let found = blocks(&source);
    assert_eq!(found.len(), 2);
    assert_eq!(source.ast["children"][0]["fence"], "~~~");
    assert_eq!(source.ast["children"][0]["lang"], "yml");
    assert_eq!(source.ast["children"][1]["fence"], "```");
    assert_eq!(source.ast["children"][1]["lang"], "toml");
}

#[test]
fn crlf_text_keeps_its_breaks_and_unicode_its_bytes() {
    let text = "para\r\n\r\n- l\r\n\r\n  ```\r\n  é中\r\n  ```\r\n";
    let source = source(text);
    let block = &blocks(&source)[0];
    let segments = mapped(&source, text, block);
    assert_eq!(segments, [("é中\r\n".to_owned(), "é中\r\n".to_owned())]);
}

#[test]
fn indented_code_blocks_are_blocks_without_a_language() {
    let text = "text\n\n    indented\n\n    more\n";
    let source = source(text);
    let block = &blocks(&source)[0];
    let node = pointer_value(&source.ast, block).unwrap();
    assert_eq!(node["fenced"], false);
    assert_eq!(node["lang"], "");
    assert_eq!(
        mapped(&source, text, block),
        [
            ("indented\n\n".to_owned(), "indented\n\n".to_owned()),
            ("more\n".to_owned(), "more\n".to_owned()),
        ]
    );
}

#[test]
fn an_unclosed_fence_at_the_end_of_the_document_reaches_the_end_of_the_text() {
    let text = "```rust\nfn a() {}";
    let source = source(text);
    let block = &blocks(&source)[0];
    let body = &source.mapping[&format!("{block}/text")];
    assert_eq!(body.range.unwrap()[1] as usize, text.len());
    let segments = body.text_map.as_ref().unwrap();
    assert_eq!(
        segments.last().unwrap().source_range[1] as usize,
        text.len()
    );
    assert_eq!(source.ast["children"][0]["closed"], false);
}

#[test]
fn an_empty_fence_has_an_empty_body_at_its_closing_line() {
    let text = "```go\n```\n";
    let source = source(text);
    let block = &blocks(&source)[0];
    let body = &source.mapping[&format!("{block}/text")];
    assert_eq!(body.range, Some([6, 6]));
    assert_eq!(body.text_map, Some(Vec::new()));
}

#[test]
fn a_fence_the_parser_re_indented_still_names_its_body() {
    let text = "- a\n\t```\n\t\tx\n\t```\n";
    let source = source(text);
    let block = &blocks(&source)[0];
    let body = &source.mapping[&format!("{block}/text")];
    assert!(body.range.is_some());
    let text_pointer = source.mapping[block.as_str()].text_pointer.as_deref();
    assert_eq!(text_pointer, Some(format!("{block}/text").as_str()));
}

#[test]
fn identical_fences_at_different_places_have_distinct_pointers() {
    let text = "```go\nx := 1\n```\n\n```go\nx := 1\n```\n";
    let source = source(text);
    let found = blocks(&source);
    assert_eq!(found.len(), 2);
    assert_ne!(found[0], found[1]);
    let first = source.mapping[&format!("{}/text", found[0])].range.unwrap();
    let second = source.mapping[&format!("{}/text", found[1])].range.unwrap();
    assert!(first[1] < second[0]);
}

#[test]
fn the_info_string_is_exposed_raw_and_split_as_rustdoc_reads_it() {
    let text =
        "```rust,ignore\n```\n\n```Rust no_run, edition2021\n```\n\n```golang\n```\n\n```\n```\n";
    let source = source(text);
    let node = |at: usize| &source.ast["children"][at];
    assert_eq!(node(0)["info"], "rust,ignore");
    assert_eq!(node(0)["lang"], "rust");
    assert_eq!(node(0)["attributes"], serde_json::json!(["ignore"]));
    assert_eq!(node(1)["lang"], "Rust");
    assert_eq!(
        node(1)["attributes"],
        serde_json::json!(["no_run", "edition2021"])
    );
    assert_eq!(node(2)["lang"], "golang");
    assert_eq!(node(3)["lang"], "");
    assert_eq!(node(3)["attributes"], serde_json::json!([]));
}

#[test]
fn links_carry_their_destination() {
    let source = source("[a](./b.md#c \"t\") and ![i](x.png)\n");
    let paragraph = &source.ast["children"][0];
    let link = &paragraph["children"][0];
    assert_eq!(link["kind"], "link");
    assert_eq!(link["url"], "./b.md#c");
    assert_eq!(link["title"], "t");
}

#[test]
fn front_matter_is_not_a_thematic_break() {
    let source = source("---\ntitle: x\n---\n\ntext\n");
    assert_eq!(source.ast["children"][0]["kind"], "frontmatter");
}

#[test]
fn every_range_is_inside_the_document_on_a_unicode_text() {
    let text = "# é中 title\n\n> quote é\n\n| a | b |\n|---|---|\n| é | 中 |\n\n- [ ] task\n";
    let _ = source(text);
}
