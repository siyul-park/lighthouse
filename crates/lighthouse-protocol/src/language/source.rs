use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A provider-specific AST and its independent source-map sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Source {
    /// Project-relative physical file path.
    pub path: String,
    /// Structurally faithful, provider-defined serialization of its parser AST.
    pub ast: Value,
    /// Original host locations keyed by canonical JSON Pointer into `ast`.
    pub mapping: BTreeMap<String, Mapping>,
}

/// Location metadata for one AST value. Missing fields are meaningful only for
/// synthetic AST values; source-bearing scalars must have a range.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Mapping {
    /// Original host UTF-8 byte extent, half-open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[u32; 2]>,
    /// JSON Pointer to a source-text string scalar inside this node's subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_pointer: Option<String>,
    /// Optional mapping from extracted UTF-8 bytes to original host bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_map: Option<Vec<TextMapSegment>>,
}

/// One half-open UTF-8 byte range in the extracted source and its original text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextMapSegment {
    pub text_range: [u32; 2],
    pub source_range: [u32; 2],
}

impl Source {
    /// Looks up a logical AST value using its canonical RFC 6901 pointer.
    pub fn value_at(&self, pointer: &str) -> Option<&Value> {
        pointer_value(&self.ast, pointer)
    }

    /// Validates pointer resolution and all supplied source ranges against the
    /// original host text. Source extraction still validates the selected
    /// scalar has a usable range.
    pub fn validate(&self, host: &str) -> Result<(), String> {
        for (pointer, entry) in &self.mapping {
            let tokens = pointer_tokens(pointer)
                .ok_or_else(|| format!("non-canonical JSON Pointer {pointer}"))?;
            let value = pointer_value(&self.ast, pointer)
                .ok_or_else(|| format!("source-map pointer {pointer} does not resolve in AST"))?;
            let range = entry
                .range
                .map(|range| validate_source_range(range, host, pointer))
                .transpose()?;

            if entry.text_pointer.is_none() && entry.text_map.is_none() && entry.range.is_none() {
                return Err(format!(
                    "source-map entry {pointer} has no mapping metadata"
                ));
            }

            if let Some(segments) = &entry.text_map {
                let range =
                    range.ok_or_else(|| format!("text_map at {pointer} has no source range"))?;
                let text = value
                    .as_str()
                    .ok_or_else(|| format!("text_map at {pointer} must address a string scalar"))?;
                validate_text_map(segments, text, host, Some(range), pointer)?;
            }

            if let Some(target) = &entry.text_pointer {
                let owner_range = range
                    .ok_or_else(|| format!("source-bearing node {pointer} has no source range"))?;
                let target_tokens = pointer_tokens(target)
                    .ok_or_else(|| format!("non-canonical text_pointer {target} at {pointer}"))?;
                if !pointer_tokens_below(&target_tokens, &tokens) {
                    return Err(format!(
                        "text_pointer {target} is not inside the subtree at {pointer}"
                    ));
                }
                if pointer_value(&self.ast, target)
                    .and_then(Value::as_str)
                    .is_none()
                {
                    return Err(format!(
                        "text_pointer {target} does not resolve to a string scalar"
                    ));
                }
                let target_entry = self
                    .mapping
                    .get(target)
                    .ok_or_else(|| format!("text_pointer {target} has no source-map entry"))?;
                let target_range = target_entry
                    .range
                    .ok_or_else(|| format!("text_pointer {target} has no source range"))?;
                let target_range = validate_source_range(target_range, host, target)?;
                if target_range[0] < owner_range[0] || target_range[1] > owner_range[1] {
                    return Err(format!(
                        "text_pointer {target} range is outside the source-bearing node {pointer}"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Coalesces adjacent byte mappings when both extents are one-to-one.
/// `starts` and `ends` contain one original-source offset per byte of `text`.
pub fn coalesce_text_map(text: &str, starts: &[u32], ends: &[u32]) -> Option<Vec<TextMapSegment>> {
    if text.len() != starts.len() || text.len() != ends.len() {
        return None;
    }
    let mut segments: Vec<TextMapSegment> = Vec::new();
    for (offset, ch) in text.char_indices() {
        let byte_end = offset + ch.len_utf8();
        let text_start = u32::try_from(offset).ok()?;
        let text_end = u32::try_from(byte_end).ok()?;
        let source_start = starts[offset];
        let source_end = ends[byte_end - 1];
        let source_width = source_end.checked_sub(source_start)?;
        if let Some(last) = segments.last_mut()
            && last.text_range[1] == text_start
            && last.source_range[1] == source_start
            && last.text_range[1].checked_sub(last.text_range[0])?
                == last.source_range[1].checked_sub(last.source_range[0])?
            && text_end.checked_sub(text_start)? == source_width
        {
            last.text_range[1] = text_end;
            last.source_range[1] = source_end;
        } else {
            segments.push(TextMapSegment {
                text_range: [text_start, text_end],
                source_range: [source_start, source_end],
            });
        }
    }
    Some(segments)
}

/// Parses a canonical JSON Pointer into its decoded path tokens.
pub fn pointer_tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    if !pointer.starts_with('/') {
        return None;
    }
    pointer
        .split('/')
        .skip(1)
        .map(decode_pointer_token)
        .collect()
}

/// Resolves a canonical JSON Pointer against a JSON-compatible value.
pub fn pointer_value<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    let tokens = pointer_tokens(pointer)?;
    let mut value = root;
    for token in tokens {
        value = match value {
            Value::Object(object) => object.get(&token)?,
            Value::Array(array) => {
                if token.is_empty() || (token.len() > 1 && token.starts_with('0')) {
                    return None;
                }
                array.get(token.parse::<usize>().ok()?)?
            }
            _ => return None,
        };
    }
    Some(value)
}

/// Appends one decoded object key or array index to a canonical JSON Pointer.
pub fn pointer_child(parent: &str, token: &str) -> String {
    format!("{parent}/{}", token.replace('~', "~0").replace('/', "~1"))
}

/// Whether `child` is strictly nested below `parent` in JSON Pointer space.
fn pointer_tokens_below(child: &[String], parent: &[String]) -> bool {
    child.len() > parent.len() && child.starts_with(parent)
}

fn decode_pointer_token(token: &str) -> Option<String> {
    let mut decoded = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next()? {
            '0' => decoded.push('~'),
            '1' => decoded.push('/'),
            _ => return None,
        }
    }
    let canonical = decoded.replace('~', "~0").replace('/', "~1");
    (canonical == token).then_some(decoded)
}

fn validate_source_range(range: [u32; 2], host: &str, pointer: &str) -> Result<[usize; 2], String> {
    let [start, end] = range.map(|value| value as usize);
    if start > end
        || end > host.len()
        || !host.is_char_boundary(start)
        || !host.is_char_boundary(end)
    {
        return Err(format!(
            "range at {pointer} is outside host text or splits UTF-8"
        ));
    }
    Ok([start, end])
}

fn validate_text_map(
    segments: &[TextMapSegment],
    text: &str,
    host: &str,
    enclosing_range: Option<[usize; 2]>,
    pointer: &str,
) -> Result<(), String> {
    let mut text_offset = 0usize;
    let mut prior_source_end = None;
    for segment in segments {
        let [text_start, text_end] = segment.text_range.map(|value| value as usize);
        let [source_start, source_end] = segment.source_range.map(|value| value as usize);
        if text_start != text_offset
            || text_start > text_end
            || text_end > text.len()
            || !text.is_char_boundary(text_start)
            || !text.is_char_boundary(text_end)
        {
            return Err(format!(
                "text_map at {pointer} has a gap, overlap, invalid range, or UTF-8 split"
            ));
        }
        if source_start > source_end
            || source_end > host.len()
            || !host.is_char_boundary(source_start)
            || !host.is_char_boundary(source_end)
            || prior_source_end.is_some_and(|previous| source_start < previous)
            || enclosing_range.is_some_and(|[start, end]| source_start < start || source_end > end)
        {
            return Err(format!(
                "text_map at {pointer} has invalid source range {source_start}..{source_end} (previous end {prior_source_end:?}, enclosing range {enclosing_range:?}, host length {})",
                host.len()
            ));
        }
        text_offset = text_end;
        prior_source_end = Some(source_end);
    }
    if text_offset != text.len() {
        return Err(format!(
            "text_map at {pointer} covers {text_offset} of {} UTF-8 bytes",
            text.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod source_ast_tests {
    use super::*;

    #[test]
    fn coalescing_text_maps_keeps_non_linear_ranges_separate() {
        let text = "é\"b";
        let starts = [10, 10, 12, 14];
        let ends = [12, 12, 14, 15];
        let segments = coalesce_text_map(text, &starts, &ends).unwrap();
        assert_eq!(
            segments,
            vec![
                TextMapSegment {
                    text_range: [0, 2],
                    source_range: [10, 12],
                },
                TextMapSegment {
                    text_range: [2, 3],
                    source_range: [12, 14],
                },
                TextMapSegment {
                    text_range: [3, 4],
                    source_range: [14, 15],
                },
            ]
        );
        assert_eq!(coalesce_text_map("x", &[], &[]), None);
        assert_eq!(coalesce_text_map("x", &[2], &[1]), None);
    }

    #[test]
    fn source_value_at_reads_the_canonical_pointer() {
        let source = Source {
            path: "sample.go".to_owned(),
            ast: serde_json::json!({"a/b": {"~key": "value"}}),
            mapping: BTreeMap::new(),
        };
        assert_eq!(
            source.value_at("/a~1b/~0key"),
            Some(&serde_json::json!("value"))
        );
    }

    #[test]
    fn pointer_tokens_decodes_canonical_escapes() {
        assert_eq!(pointer_tokens("/a~1b/~0key").unwrap(), ["a/b", "~key"]);
        assert!(pointer_tokens("/bad~2token").is_none());
    }

    #[test]
    fn pointer_value_resolves_an_array_element() {
        let value = serde_json::json!({"items": ["zero", "one"]});
        assert_eq!(
            pointer_value(&value, "/items/1"),
            Some(&serde_json::json!("one"))
        );
    }

    #[test]
    fn pointer_child_escapes_pointer_tokens() {
        assert_eq!(pointer_child("/items", "a/b~c"), "/items/a~1b~0c");
    }

    #[test]
    fn source_ast_validates_json_pointers_and_segment_maps() {
        let ast = Source {
            path: "config.yaml".to_owned(),
            ast: serde_json::json!({"a/b": {"~key": "éx"}}),
            mapping: BTreeMap::from([
                (
                    "".to_owned(),
                    Mapping {
                        range: Some([0, 3]),
                        ..Default::default()
                    },
                ),
                (
                    "/a~1b".to_owned(),
                    Mapping {
                        range: Some([0, 3]),
                        text_pointer: Some("/a~1b/~0key".to_owned()),
                        ..Default::default()
                    },
                ),
                (
                    "/a~1b/~0key".to_owned(),
                    Mapping {
                        range: Some([0, 3]),
                        text_map: Some(vec![
                            TextMapSegment {
                                text_range: [0, 2],
                                source_range: [0, 2],
                            },
                            TextMapSegment {
                                text_range: [2, 3],
                                source_range: [2, 3],
                            },
                        ]),
                        ..Default::default()
                    },
                ),
            ]),
        };
        assert!(ast.validate("éx").is_ok());

        let mut malformed = ast.clone();
        malformed.mapping.get_mut("/a~1b/~0key").unwrap().text_map = Some(vec![TextMapSegment {
            text_range: [1, 2],
            source_range: [0, 1],
        }]);
        assert!(malformed.validate("éx").unwrap_err().contains("text_map"));

        let mut malformed = ast;
        malformed.mapping.get_mut("/a~1b").unwrap().text_pointer = Some("/missing".to_owned());
        assert!(
            malformed
                .validate("éx")
                .unwrap_err()
                .contains("text_pointer")
        );
    }

    #[test]
    fn text_map_requires_the_whole_source_scalar_range() {
        let source = Source {
            path: "host.yaml".to_owned(),
            ast: serde_json::json!({"source":"x"}),
            mapping: BTreeMap::from([(
                "/source".to_owned(),
                Mapping {
                    text_map: Some(vec![TextMapSegment {
                        text_range: [0, 1],
                        source_range: [0, 1],
                    }]),
                    ..Default::default()
                },
            )]),
        };
        assert!(
            source
                .validate("x")
                .unwrap_err()
                .contains("text_map at /source has no source range")
        );
    }

    #[test]
    fn text_pointer_requires_a_ranged_owner_that_contains_its_source_scalar() {
        let source = || Source {
            path: "host.go".to_owned(),
            ast: serde_json::json!({"expr":{"source":"abc"}}),
            mapping: BTreeMap::from([
                (
                    "".to_owned(),
                    Mapping {
                        range: Some([0, 5]),
                        ..Default::default()
                    },
                ),
                (
                    "/expr".to_owned(),
                    Mapping {
                        range: Some([0, 3]),
                        text_pointer: Some("/expr/source".to_owned()),
                        ..Default::default()
                    },
                ),
                (
                    "/expr/source".to_owned(),
                    Mapping {
                        range: Some([0, 3]),
                        ..Default::default()
                    },
                ),
            ]),
        };

        assert!(source().validate("abcde").is_ok());

        let mut missing_owner_range = source();
        missing_owner_range.mapping.get_mut("/expr").unwrap().range = None;
        assert!(
            missing_owner_range
                .validate("abcde")
                .unwrap_err()
                .contains("source-bearing node /expr has no source range")
        );

        let mut outside_owner = source();
        outside_owner.mapping.get_mut("/expr/source").unwrap().range = Some([2, 5]);
        assert!(
            outside_owner
                .validate("abcde")
                .unwrap_err()
                .contains("range is outside the source-bearing node /expr")
        );
    }

    #[test]
    fn source_ast_rejects_bad_pointer_escapes_and_missing_scalar_ranges() {
        let ast = Source {
            path: "host.yaml".to_owned(),
            ast: serde_json::json!({"source": "x"}),
            mapping: BTreeMap::from([(
                "/~2".to_owned(),
                Mapping {
                    range: Some([0, 1]),
                    ..Default::default()
                },
            )]),
        };
        assert!(
            ast.validate("x")
                .unwrap_err()
                .contains("non-canonical JSON Pointer")
        );

        let ast = Source {
            path: "host.yaml".to_owned(),
            ast: serde_json::json!({"source": "x"}),
            mapping: BTreeMap::from([
                (
                    "".to_owned(),
                    Mapping {
                        range: Some([0, 1]),
                        text_pointer: Some("/source".to_owned()),
                        ..Default::default()
                    },
                ),
                ("/source".to_owned(), Mapping::default()),
            ]),
        };
        assert!(ast.validate("x").unwrap_err().contains("no source range"));
    }
}
