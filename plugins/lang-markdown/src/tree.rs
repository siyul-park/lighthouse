//! The nodes of a Markdown document as JSON, and where each lies.

use std::collections::BTreeMap;

use comrak::{
    Arena, Options,
    nodes::{ListType, Node, NodeValue, TableAlignment},
    parse_document,
};
use lighthouse_protocol::{HostError, HostTree, Mapping, pointer_child};
use serde_json::{Map, Value, json};

use crate::{
    body::{self, Block},
    fence::Info,
    lines::Lines,
};

/// How deep blocks may nest before the tree is refused rather than walked.
const MAX_DEPTH: usize = 256;

struct Walk<'t> {
    lines: Lines<'t>,
    mapping: BTreeMap<String, Mapping>,
}

impl Walk<'_> {
    /// The JSON of `node` at `pointer`, with its place in the mapping.
    fn node(&mut self, node: Node<'_>, pointer: &str, depth: usize) -> Result<Value, HostError> {
        if depth > MAX_DEPTH {
            return Err(HostError::Tree(format!(
                "blocks nest deeper than {MAX_DEPTH} levels at {pointer}"
            )));
        }
        let data = node.data();
        let sourcepos = data.sourcepos;
        let mut object = Map::new();
        object.insert("kind".to_owned(), json!(data.value.xml_node_name()));
        let mut range = self.lines.range(
            (sourcepos.start.line, sourcepos.start.column),
            (sourcepos.end.line, sourcepos.end.column),
        );
        let mut entry = Mapping::default();
        match &data.value {
            NodeValue::CodeBlock(code) => {
                let body =
                    self.code_block(&mut object, code, sourcepos.start.line, sourcepos.end.line);
                let text_pointer = pointer_child(pointer, "text");
                self.mapping.insert(
                    text_pointer.clone(),
                    Mapping {
                        range: Some(body.range.map(|at| at as u32)),
                        text_pointer: None,
                        text_map: body.map,
                    },
                );
                // A fence left open runs to the end of its last line.
                if let Some(whole) = range.as_mut() {
                    whole[1] = whole[1].max(body.range[1]);
                }
                entry.text_pointer = Some(text_pointer);
            }
            value => fields(&mut object, value),
        }
        let mut children = Vec::new();
        for (index, child) in node.children().enumerate() {
            let pointer = pointer_child(&pointer_child(pointer, "children"), &index.to_string());
            children.push(self.node(child, &pointer, depth + 1)?);
        }
        if !children.is_empty() {
            object.insert("children".to_owned(), Value::Array(children));
        }
        if let Some(range) = range {
            entry.range = Some(range.map(|at| at as u32));
            self.mapping.insert(pointer.to_owned(), entry);
        }
        Ok(Value::Object(object))
    }

    /// The fields of a code block, and where its body lies.
    fn code_block(
        &self,
        object: &mut Map<String, Value>,
        code: &comrak::nodes::NodeCodeBlock,
        start: usize,
        end: usize,
    ) -> body::Body {
        let info = Info::of(&code.info);
        object.insert("fenced".to_owned(), json!(code.fenced));
        if code.fenced {
            object.insert(
                "fence".to_owned(),
                json!(
                    char::from(code.fence_char)
                        .to_string()
                        .repeat(code.fence_length)
                ),
            );
        }
        object.insert("closed".to_owned(), json!(code.closed));
        object.insert("info".to_owned(), json!(code.info));
        object.insert("lang".to_owned(), json!(info.lang));
        object.insert("attributes".to_owned(), json!(info.attributes));
        object.insert("key".to_owned(), json!(info.key));
        object.insert("text".to_owned(), json!(code.literal));
        body::locate(
            &self.lines,
            &Block {
                fenced: code.fenced,
                closed: code.closed,
                lines: (start, end),
                literal: &code.literal,
            },
        )
    }
}

/// The fields of a node besides its kind and children: what the parser keeps
/// of it.
fn fields(object: &mut Map<String, Value>, value: &NodeValue) {
    let mut set = |key: &str, value: Value| {
        object.insert(key.to_owned(), value);
    };
    match value {
        NodeValue::List(list) | NodeValue::Item(list) => {
            let ordered = list.list_type == ListType::Ordered;
            set("ordered", json!(ordered));
            if ordered {
                set("start", json!(list.start));
                set(
                    "delimiter",
                    json!(format!("{:?}", list.delimiter).to_lowercase()),
                );
            } else {
                set("bullet", json!(char::from(list.bullet_char).to_string()));
            }
            set("tight", json!(list.tight));
        }
        NodeValue::Heading(heading) => {
            set("level", json!(heading.level));
            set("setext", json!(heading.setext));
        }
        NodeValue::Link(link) | NodeValue::Image(link) => {
            set("url", json!(link.url));
            set("title", json!(link.title));
        }
        NodeValue::Text(text) => set("text", json!(text)),
        NodeValue::Code(code) => set("text", json!(code.literal)),
        NodeValue::HtmlInline(text) | NodeValue::FrontMatter(text) => set("text", json!(text)),
        NodeValue::HtmlBlock(html) => set("text", json!(html.literal)),
        NodeValue::Table(table) => {
            set(
                "alignments",
                json!(table.alignments.iter().map(align).collect::<Vec<_>>()),
            );
        }
        NodeValue::TableRow(header) => set("header", json!(header)),
        NodeValue::FootnoteDefinition(note) => set("name", json!(note.name)),
        NodeValue::FootnoteReference(note) => set("name", json!(note.name)),
        NodeValue::TaskItem(task) => set("checked", json!(task.symbol.is_some())),
        _ => {}
    }
}

fn align(alignment: &TableAlignment) -> &'static str {
    match alignment {
        TableAlignment::None => "none",
        TableAlignment::Left => "left",
        TableAlignment::Center => "center",
        TableAlignment::Right => "right",
    }
}

/// The tree of `text`: the document and the blocks and inlines below it.
pub(crate) fn parse(text: &str) -> Result<HostTree, HostError> {
    let arena = Arena::new();
    let root = parse_document(&arena, text, &options());
    let mut walk = Walk {
        lines: Lines::new(text),
        mapping: BTreeMap::new(),
    };
    let ast = walk.node(root, "", 0)?;
    walk.mapping.insert(
        String::new(),
        Mapping {
            range: Some([0, text.len() as u32]),
            text_pointer: None,
            text_map: None,
        },
    );
    Ok(HostTree {
        ast,
        mapping: walk.mapping,
        comments: Vec::new(),
    })
}

/// The options the tree is read with: GitHub Flavored Markdown and the front
/// matter that starts a document.
fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options
}
