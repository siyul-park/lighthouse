use lighthouse_protocol::{Source, coalesce_text_map};
use serde_json::json;

use super::*;

fn tree_of(host: &str) -> HostTree {
    tree::parse(host).unwrap()
}

#[test]
fn a_float_json_cannot_hold_is_kept_as_its_text() {
    let tree = tree_of("value = nan\nbig = inf\ndate = 1979-05-27T07:32:00Z\n");
    assert_eq!(tree.ast["value"], "nan");
    assert_eq!(tree.ast["big"], "inf");
    assert_eq!(tree.ast["date"], "1979-05-27T07:32:00Z");
    assert!(tree.mapping.contains_key("/value"));
}

#[test]
fn serialized_ast_keeps_decoded_scalar_and_json_pointer_source_map() {
    let host = "where = \"x > 1\"\nlimit = 10\n";
    let tree = tree_of(host);
    let ast = Source {
        path: "config.toml".to_owned(),
        ast: tree.ast,
        mapping: tree.mapping,
    };

    assert_eq!(ast.value_at("/where").unwrap(), "x > 1");
    let start = host.find("x > 1").unwrap();
    let entry = ast.mapping.get("/where").unwrap();
    assert_eq!(entry.range, Some([8, 15]));
    let map = entry.text_map.as_ref().unwrap();
    assert_eq!(map.first().unwrap().source_range[0] as usize, start);
    assert_eq!(
        map.last().unwrap().source_range[1] as usize,
        start + "x > 1".len()
    );
    assert_eq!(ast.value_at("/limit").unwrap(), 10);
    ast.validate(host).unwrap();
}

#[test]
fn tables_arrays_of_tables_and_inline_values_nest() {
    let host = "[package]\nname = \"x\"\n\n[[bin]]\nname = \"a\"\n\n[[bin]]\nname = \"b\"\nflags = [1, 2]\nopt = { on = true }\n";
    let tree = tree_of(host);
    assert_eq!(
        tree.ast,
        json!({
            "package": {"name": "x"},
            "bin": [
                {"name": "a"},
                {"name": "b", "flags": [1, 2], "opt": {"on": true}},
            ],
        })
    );
    for pointer in ["/bin/1/name", "/bin/1/flags/1", "/bin/1/opt/on", "/bin/0"] {
        assert!(tree.mapping.contains_key(pointer), "{pointer}");
    }
    Source {
        path: "Cargo.toml".to_owned(),
        ast: tree.ast,
        mapping: tree.mapping,
    }
    .validate(host)
    .unwrap();
}

#[test]
fn escapes_and_multiline_strings_map_to_their_source_bytes() {
    let host = "a = \"x\\u00e9y\"\nb = '''\nline one\nline two'''\n";
    let tree = tree_of(host);
    assert_eq!(tree.ast["a"], "xéy");
    assert_eq!(tree.ast["b"], "line one\nline two");
    let escaped = tree.mapping["/a"].text_map.as_ref().unwrap();
    assert!(escaped.len() > 1, "{escaped:?}");
    Source {
        path: "s.toml".to_owned(),
        ast: tree.ast,
        mapping: tree.mapping,
    }
    .validate(host)
    .unwrap();
}

#[test]
fn an_invalid_document_is_a_syntax_error() {
    assert!(matches!(tree::parse("a = \n"), Err(HostError::Syntax(_))));
}

#[test]
fn invalid_text_maps_are_rejected() {
    assert!(coalesce_text_map("x", &[], &[]).is_none());
    assert!(coalesce_text_map("x", &[2], &[1]).is_none());
}

#[test]
fn escaped_unicode_keeps_a_non_linear_segment_separate() {
    let text = "雪x";
    let starts = [1, 1, 1, 7];
    let ends = [7, 7, 7, 8];
    let segments = coalesce_text_map(text, &starts, &ends).unwrap();

    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].text_range, [0, 3]);
    assert_eq!(segments[0].source_range, [1, 7]);
    assert_eq!(segments[1].text_range, [3, 4]);
    assert_eq!(segments[1].source_range, [7, 8]);
}
