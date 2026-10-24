use std::{collections::HashMap, path::Path};

use lighthouse_model::ucm::{SymbolId, SymbolKind};
use lighthouse_plugin::Source;

use super::host::*;

struct YamlParser<'a> {
    text: &'a str,
    lines: Vec<&'a str>,
    offsets: Vec<usize>,
    scalars: Vec<Scalar>,
    stack: Vec<(usize, String)>,
    sequence_ids: HashMap<Vec<String>, usize>,
    sequence_starts: HashMap<usize, usize>,
    next_item: usize,
    item_ranges: HashMap<usize, (usize, usize)>,
    doc: usize,
    doc_has_content: bool,
}

struct YamlEntry<'a> {
    index: usize,
    line: &'a str,
    indent: usize,
    effective_indent: usize,
    parent: Vec<String>,
    path: Vec<String>,
    item: Option<usize>,
    key: &'a str,
    value_begin: usize,
    leading: usize,
    raw_value: &'a str,
}

impl<'a> YamlParser<'a> {
    fn new(text: &'a str) -> Self {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let mut offsets = Vec::with_capacity(lines.len());
        let mut at = 0usize;
        for line in &lines {
            offsets.push(at);
            at += line.len();
        }
        Self {
            text,
            lines,
            offsets,
            scalars: Vec::new(),
            stack: Vec::new(),
            sequence_ids: HashMap::new(),
            sequence_starts: HashMap::new(),
            next_item: 0,
            item_ranges: HashMap::new(),
            doc: 0,
            doc_has_content: false,
        }
    }

    fn parse(mut self) -> (Vec<Scalar>, HashMap<usize, (usize, usize)>) {
        let mut index = 0usize;
        while index < self.lines.len() {
            self.parse_line(&mut index);
        }
        for (id, start) in self.sequence_starts {
            let range = self.item_ranges.entry(id).or_insert((start, start));
            range.0 = start;
        }
        (self.scalars, self.item_ranges)
    }

    fn parse_line(&mut self, index: &mut usize) {
        let line = self.lines[*index].trim_end_matches(['\r', '\n']);
        let indent = line.bytes().take_while(|byte| *byte == b' ').count();
        let content = &line[indent..];
        if self.document_marker(content) {
            *index += 1;
            return;
        }
        if content.is_empty() || content.starts_with('#') || content == "..." {
            *index += 1;
            return;
        }
        self.doc_has_content = true;
        let Some(entry) = self.entry(*index, line, indent, content) else {
            *index += 1;
            return;
        };
        if entry.raw_value.is_empty() {
            self.parse_empty(&entry, index);
        } else if entry.raw_value.starts_with('|') || entry.raw_value.starts_with('>') {
            self.parse_block(entry, index);
        } else {
            self.parse_inline(entry);
            *index += 1;
        }
    }

    fn document_marker(&mut self, content: &str) -> bool {
        if content != "---" {
            return false;
        }
        if self.doc_has_content {
            self.doc += 1;
        }
        self.doc_has_content = false;
        self.stack.clear();
        self.sequence_ids.clear();
        true
    }

    fn entry<'b>(
        &mut self,
        index: usize,
        line: &'b str,
        indent: usize,
        content: &'b str,
    ) -> Option<YamlEntry<'b>> {
        let mut list_item = false;
        let mut key_offset = indent;
        let mut effective_indent = indent;
        let mut rest = content;
        if let Some(after) = content.strip_prefix("- ") {
            list_item = true;
            key_offset += 2;
            effective_indent += 2;
            rest = after;
        }
        let (key, colon_end) = yaml_key(rest)?;
        while self
            .stack
            .last()
            .is_some_and(|(level, _)| *level >= effective_indent)
        {
            self.stack.pop();
        }
        let parent: Vec<String> = self.stack.iter().map(|(_, key)| key.clone()).collect();
        let item = if list_item {
            self.next_item += 1;
            let id = self.next_item;
            self.sequence_ids.insert(parent.clone(), id);
            self.sequence_starts.insert(id, self.offsets[index]);
            self.item_ranges
                .insert(id, (self.offsets[index], self.offsets[index] + line.len()));
            Some(id)
        } else {
            self.sequence_ids.get(&parent).copied()
        };
        let path: Vec<String> = parent.iter().cloned().chain([key.to_owned()]).collect();
        let value_text = &rest[colon_end..];
        let leading = value_text.len() - value_text.trim_start().len();
        let raw_value = value_text.trim_start();
        Some(YamlEntry {
            index,
            line,
            indent,
            effective_indent,
            parent,
            path,
            item,
            key,
            value_begin: key_offset + colon_end,
            leading,
            raw_value,
        })
    }

    fn parse_empty(&mut self, entry: &YamlEntry<'_>, index: &mut usize) {
        let is_params = entry.key == "params"
            && (path_starts_with(&entry.path, &["spec", "check", "variables", "params"])
                || path_starts_with(&entry.path, &["spec", "variables", "params"]));
        if !is_params {
            self.stack
                .push((entry.effective_indent, entry.key.to_owned()));
            *index += 1;
            return;
        }
        let item = self.sequence_ids.get(&entry.parent).copied();
        let (mapped, end_offset, next_line) = block_params(
            &self.lines,
            &self.offsets,
            *index + 1,
            entry.indent,
            self.text,
        );
        let source_end = end_offset.max(self.offsets[*index] + entry.line.len());
        self.scalars.push(Scalar {
            doc: self.doc,
            path: entry.path.clone(),
            key: entry.key.to_owned(),
            value: mapped,
            source_start: self.offsets[*index],
            source_end,
            item,
        });
        if let Some(item) = item {
            let range = self
                .item_ranges
                .entry(item)
                .or_insert((self.offsets[*index], source_end));
            range.1 = range.1.max(source_end);
        }
        *index = next_line;
    }

    fn parse_block(&mut self, entry: YamlEntry<'_>, index: &mut usize) {
        let block_start = *index;
        *index += 1;
        let mut body = Vec::new();
        while *index < self.lines.len() {
            let candidate = self.lines[*index].trim_end_matches(['\r', '\n']);
            let candidate_indent = candidate.bytes().take_while(|byte| *byte == b' ').count();
            if !candidate.trim().is_empty() && candidate_indent <= entry.indent {
                break;
            }
            body.push((candidate, self.offsets[*index]));
            *index += 1;
        }
        let mapped = block_scalar(self.text, entry.raw_value, entry.indent, &body);
        let source_end = body
            .last()
            .map(|(line, start)| start + line.len())
            .unwrap_or(self.offsets[block_start] + entry.line.len());
        self.push_scalar(entry, mapped, source_end, block_start);
    }

    fn parse_inline(&mut self, entry: YamlEntry<'_>) {
        let raw_end_trimmed = strip_yaml_comment(entry.raw_value).trim_end();
        let start_trimmed = entry.raw_value.len() - entry.raw_value.trim_start().len();
        let actual_start =
            self.offsets[entry.index] + entry.value_begin + entry.leading + start_trimmed;
        let mapped = inline_scalar(self.text, raw_end_trimmed, actual_start);
        let source_end = if raw_end_trimmed.is_empty() {
            actual_start
        } else {
            actual_start + raw_end_trimmed.len()
        };
        let index = entry.index;
        self.push_scalar(entry, mapped, source_end, index);
    }

    fn push_scalar(
        &mut self,
        entry: YamlEntry<'_>,
        value: MappedText,
        source_end: usize,
        start: usize,
    ) {
        self.scalars.push(Scalar {
            doc: self.doc,
            path: entry.path,
            key: entry.key.to_owned(),
            value,
            source_start: self.offsets[start],
            source_end,
            item: entry.item,
        });
        if let Some(item) = entry.item {
            let range = self
                .item_ranges
                .entry(item)
                .or_insert((self.offsets[start], source_end));
            range.0 = range.0.min(self.offsets[start]);
            range.1 = range.1.max(source_end);
        }
    }
}

pub(super) fn quoted_scalar(
    host: &str,
    raw: &str,
    source_start: usize,
    value: String,
) -> MappedText {
    let quote = raw.chars().next().unwrap_or('"');
    let mut starts = Vec::with_capacity(value.len());
    let mut ends = Vec::with_capacity(value.len());
    let mut out_index = 0usize;
    let mut raw_index = quote.len_utf8();
    let raw_end = raw.len().saturating_sub(quote.len_utf8());
    while raw_index < raw_end && out_index < value.len() {
        let source_at = source_start + raw_index;
        let ch = raw[raw_index..].chars().next().unwrap_or('\0');
        let (written, consumed) =
            if quote == '\'' && ch == '\'' && raw[raw_index + 1..].starts_with('\'') {
                ("'".to_owned(), 2)
            } else if quote == '"' && ch == '\\' {
                let escaped = raw[raw_index + 1..].chars().next().unwrap_or('\0');
                let consumed = match escaped {
                    'x' => 4,
                    'u' => 6,
                    'U' => 10,
                    _ => 2,
                };
                let next = value[out_index..].chars().next().unwrap_or('\0');
                (next.to_string(), consumed.min(raw_end - raw_index))
            } else {
                (ch.to_string(), ch.len_utf8())
            };
        for _ in written.bytes() {
            starts.push(source_at);
            ends.push(source_at + consumed);
        }
        out_index += written.len();
        raw_index += consumed;
    }
    if starts.len() != value.len() {
        starts.clear();
        ends.clear();
        let mut raw_at = source_start + quote.len_utf8();
        for ch in value.chars() {
            starts.extend(std::iter::repeat_n(raw_at, ch.len_utf8()));
            ends.extend(std::iter::repeat_n(raw_at + ch.len_utf8(), ch.len_utf8()));
            raw_at += ch.len_utf8();
        }
    }
    MappedText {
        text: value,
        starts,
        ends,
        source_start: source_start + quote.len_utf8(),
        source_end: source_start + raw.len().saturating_sub(quote.len_utf8()),
        host: host.to_owned(),
    }
}

pub(super) fn yaml_draft(source: &Source) -> FileDraft {
    let mut draft = FileDraft::empty(source);
    let (scalars, item_ranges) = yaml_scalars(source.text);
    let docs = yaml_docs(&scalars, &source.file.path);
    let mut snippets = Vec::new();

    for doc in &docs {
        add_document(
            &mut draft,
            source,
            &scalars,
            &item_ranges,
            doc,
            &mut snippets,
        );
    }
    draft
        .snippets
        .extend(snippets.into_iter().map(|(_, snippet)| snippet));
    draft
}

fn add_document(
    draft: &mut FileDraft,
    source: &Source,
    scalars: &[Scalar],
    item_ranges: &HashMap<usize, (usize, usize)>,
    doc: &Doc,
    snippets: &mut Vec<(usize, Snippet)>,
) {
    let root = scalars.iter().filter(|scalar| scalar.doc == doc.index);
    let cel_check = root
        .clone()
        .any(|scalar| scalar.path == ["spec", "check", "type"] && scalar.value.text == "cel");
    let cel_fix = root
        .clone()
        .any(|scalar| scalar.path == ["spec", "fix", "type"] && scalar.value.text == "cel");

    if doc.kind == "Pack" {
        snippets.extend(add_variables(
            draft,
            source,
            doc,
            scalars,
            item_ranges,
            &["spec", "variables"],
            true,
        ));
    }
    if cel_check {
        snippets.extend(add_variables(
            draft,
            source,
            doc,
            scalars,
            item_ranges,
            &["spec", "check", "variables"],
            false,
        ));
        add_check_snippets(draft, doc, scalars);
    }
    if cel_fix
        && let Some(scalar) = scalars
            .iter()
            .find(|scalar| scalar.doc == doc.index && scalar.path == ["spec", "fix", "edits"])
    {
        add_snippet(draft, doc, scalar, "fix.edits", None, 0);
    }
}

fn add_check_snippets(draft: &mut FileDraft, doc: &Doc, scalars: &[Scalar]) {
    for scalar in scalars.iter().filter(|scalar| scalar.doc == doc.index) {
        let path = scalar.path.as_slice();
        if check_expression_path(path) {
            add_snippet(draft, doc, scalar, path_key(path), None, 0);
        } else if path == ["spec", "check", "evidence"] {
            add_snippet(draft, doc, scalar, "evidence", None, 0);
        } else if path == ["spec", "check", "message"] {
            add_message_snippets(draft, doc, scalar);
        }
    }
}

fn check_expression_path(path: &[String]) -> bool {
    path == ["spec", "check", "where"]
        || path == ["spec", "check", "abstain"]
        || path_starts_with(path, &["spec", "check", "identity"])
        || path_starts_with(path, &["spec", "check", "at"])
        || path_starts_with(path, &["spec", "check", "evidence"]) && path.len() > 3
}

fn add_message_snippets(draft: &mut FileDraft, doc: &Doc, scalar: &Scalar) {
    for (index, template) in template_expressions(&scalar.value).into_iter().enumerate() {
        let key = format!("message.{index}");
        add_snippet(draft, doc, scalar, &key, Some(template), index);
    }
}

fn yaml_docs(scalars: &[Scalar], path: &Path) -> Vec<Doc> {
    let max = scalars.iter().map(|scalar| scalar.doc).max().unwrap_or(0);
    (0..=max)
        .map(|index| {
            let kind = scalars
                .iter()
                .find(|scalar| scalar.doc == index && scalar.path == ["kind"])
                .map(|scalar| scalar.value.text.clone())
                .unwrap_or_else(|| "Project".to_owned());
            let name = scalars
                .iter()
                .find(|scalar| scalar.doc == index && scalar.path == ["metadata", "name"])
                .map(|scalar| scalar.value.text.clone())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            Doc {
                index,
                id: format!("{kind}:{name}"),
                kind,
            }
        })
        .collect()
}

fn add_variables(
    draft: &mut FileDraft,
    source: &Source,
    doc: &Doc,
    scalars: &[Scalar],
    item_ranges: &HashMap<usize, (usize, usize)>,
    prefix: &[&str],
    public: bool,
) -> Vec<(usize, Snippet)> {
    let mut snippets = Vec::new();
    let expression_path: Vec<String> = prefix
        .iter()
        .map(|part| (*part).to_owned())
        .chain(["expression".to_owned()])
        .collect();
    for expression in scalars
        .iter()
        .filter(|scalar| scalar.doc == doc.index && scalar.path == expression_path)
    {
        let Some(item) = expression.item else {
            continue;
        };
        let name_path: Vec<String> = prefix
            .iter()
            .map(|part| (*part).to_owned())
            .chain(["name".to_owned()])
            .collect();
        let Some(name) = scalars.iter().find(|scalar| {
            scalar.doc == doc.index && scalar.item == Some(item) && scalar.path == name_path
        }) else {
            continue;
        };
        let variable_name = name.value.text.trim().to_owned();
        if variable_name.is_empty() {
            continue;
        }
        let module = &draft.module;
        let id = SymbolId::new(
            module,
            &[&doc.id, "variables"],
            &variable_name,
            SymbolKind::Var,
        );
        let (start, end) = item_ranges
            .get(&item)
            .copied()
            .unwrap_or((name.source_start, expression.source_end));
        let params_path: Vec<String> = prefix
            .iter()
            .map(|part| (*part).to_owned())
            .chain(["params".to_owned()])
            .collect();
        let params = scalars
            .iter()
            .find(|scalar| {
                scalar.doc == doc.index && scalar.item == Some(item) && scalar.path == params_path
            })
            .map(|scalar| parameters(source, scalar, module, &doc.id, &variable_name, &id))
            .unwrap_or_default();

        draft.variables.push(VariableDecl {
            doc_id: doc.id.clone(),
            name: variable_name.clone(),
            public,
            id: id.clone(),
            span: name.value.span(0, name.value.text.len(), source.text),
            extent: span(source.text, start, end),
            params: params.clone(),
        });

        let snippet = Snippet {
            doc_id: doc.id.clone(),
            id: id.clone(),
            source: expression.value.clone(),
            name: variable_name,
            params,
            declared: false,
        };
        snippets.push((expression.source_start, snippet));
    }
    snippets
}

fn parameters(
    source: &Source,
    scalar: &Scalar,
    module: &str,
    doc_id: &str,
    variable: &str,
    owner: &SymbolId,
) -> Vec<Parameter> {
    let mapped = &scalar.value;
    let Ok(names) = serde_norway::from_str::<Vec<String>>(&mapped.text) else {
        return Vec::new();
    };
    let mut params = Vec::with_capacity(names.len());
    let mut cursor = 0;
    for name in names {
        let Some(relative) = mapped.text[cursor..].find(&name) else {
            continue;
        };
        let start = cursor + relative;
        let end = start + name.len();
        let span = mapped.span(start, end, source.text);
        let id = SymbolId::new(
            module,
            &[doc_id, "variables", variable],
            &name,
            SymbolKind::Var,
        );
        params.push(Parameter {
            name,
            id,
            owner: owner.clone(),
            span,
        });
        cursor = end;
    }
    params
}

fn path_key(path: &[String]) -> &str {
    path.last().map(String::as_str).unwrap_or("expression")
}

fn template_expressions(value: &MappedText) -> Vec<MappedText> {
    let mut expressions = Vec::new();
    let mut from = 0;
    while let Some(open_rel) = value.text[from..].find("{{") {
        let open = from + open_rel;
        let body_start = open + 2;
        let Some(close_rel) = value.text[body_start..].find("}}") else {
            break;
        };
        let close = body_start + close_rel;
        let body = &value.text[body_start..close];
        let trim_start = body.len() - body.trim_start().len();
        let trim_end = body.trim_end().len();
        if trim_start < trim_end {
            expressions.push(value.slice(body_start + trim_start, body_start + trim_end));
        }
        from = close + 2;
    }
    expressions
}

fn yaml_scalars(text: &str) -> (Vec<Scalar>, HashMap<usize, (usize, usize)>) {
    YamlParser::new(text).parse()
}

fn block_params(
    lines: &[&str],
    offsets: &[usize],
    mut line_index: usize,
    parent_indent: usize,
    host: &str,
) -> (MappedText, usize, usize) {
    let mut pieces = Vec::new();
    let mut source_end = offsets
        .get(line_index.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    while line_index < lines.len() {
        let line = lines[line_index].trim_end_matches(['\r', '\n']);
        let indent = line.bytes().take_while(|byte| *byte == b' ').count();
        let content = &line[indent..];
        if !content.trim().is_empty() && indent <= parent_indent {
            break;
        }
        if content.trim().is_empty() || content.starts_with('#') {
            line_index += 1;
            continue;
        }
        let Some(value) = content.strip_prefix("- ") else {
            break;
        };
        let raw = strip_yaml_comment(value).trim_end();
        let leading = value.len() - value.trim_start().len();
        let raw_start = offsets[line_index] + indent + 2 + leading;
        pieces.push((raw.to_owned(), raw_start));
        source_end = offsets[line_index] + line.len();
        line_index += 1;
    }

    let mut value = String::from("[");
    let mut starts = vec![pieces.first().map_or(source_end, |(_, at)| *at)];
    let mut ends = starts.clone();
    for (index, (raw, at)) in pieces.iter().enumerate() {
        if index > 0 {
            value.push_str(", ");
            let separator = at.saturating_sub(1);
            starts.extend([separator, separator]);
            ends.extend([separator + 1, separator + 1]);
        }
        value.push_str(raw);
        starts.extend((0..raw.len()).map(|offset| at + offset));
        ends.extend((0..raw.len()).map(|offset| at + offset + 1));
    }
    value.push(']');
    let bracket_at = pieces.first().map_or(source_end, |(_, at)| *at);
    starts.push(bracket_at);
    ends.push(bracket_at + 1);
    (
        MappedText {
            text: value,
            starts,
            ends,
            source_start: bracket_at,
            source_end,
            host: host.to_owned(),
        },
        source_end,
        line_index,
    )
}

fn yaml_key(value: &str) -> Option<(&str, usize)> {
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('"'), '\\') => escaped = true,
            (Some(q), found) if found == q => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, ':')
                if value[at + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace) =>
            {
                let key = value[..at].trim();
                if key.is_empty() {
                    return None;
                }
                let start = value.find(key).unwrap_or(0);
                return Some((&value[start..start + key.len()], at + 1));
            }
            _ => {}
        }
    }
    None
}

fn strip_yaml_comment(value: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('"'), '\\') => escaped = true,
            (Some(q), found) if found == q => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, '#') if at == 0 || value.as_bytes()[at - 1].is_ascii_whitespace() => {
                return &value[..at];
            }
            _ => {}
        }
    }
    value
}

fn inline_scalar(host: &str, raw: &str, source_start: usize) -> MappedText {
    let raw = raw.trim_end();
    if (raw.starts_with('"') || raw.starts_with('\''))
        && let Ok(value) = serde_norway::from_str::<String>(raw)
    {
        return quoted_scalar(host, raw, source_start, value);
    }
    MappedText::raw(host, raw, source_start)
}

fn block_scalar(
    host: &str,
    header: &str,
    key_indent: usize,
    lines: &[(&str, usize)],
) -> MappedText {
    let style = header.chars().next().unwrap_or('>');
    let explicit_indent = header
        .chars()
        .find(|ch| ch.is_ascii_digit())
        .and_then(|ch| ch.to_digit(10));
    let content_indent = explicit_indent
        .map(|indent| key_indent + indent as usize)
        .unwrap_or_else(|| {
            lines
                .iter()
                .filter(|(line, _)| !line.trim().is_empty())
                .map(|(line, _)| line.bytes().take_while(|b| *b == b' ').count())
                .min()
                .unwrap_or(key_indent + 1)
        });
    let mut parts: Vec<(String, Vec<usize>, Vec<usize>, usize)> = lines
        .iter()
        .map(|(line, offset)| {
            let indent = line.bytes().take_while(|b| *b == b' ').count();
            let strip = indent.min(content_indent);
            let content = line.get(strip..).unwrap_or("").to_owned();
            let start = offset + strip;
            let starts = (0..content.len()).map(|i| start + i).collect();
            let ends = (0..content.len()).map(|i| start + i + 1).collect();
            (content, starts, ends, indent)
        })
        .collect();

    while parts.last().is_some_and(|(line, _, _, _)| line.is_empty()) && header.ends_with('-') {
        parts.pop();
    }
    let mut text = String::new();
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for index in 0..parts.len() {
        let (line, line_starts, line_ends, line_indent) = &parts[index];
        text.push_str(line);
        starts.extend(line_starts);
        ends.extend(line_ends);
        if index + 1 == parts.len() {
            continue;
        }
        let (next, _, _, next_indent) = &parts[index + 1];
        let fold = style == '>'
            && !line.is_empty()
            && !next.is_empty()
            && *line_indent <= content_indent
            && *next_indent <= content_indent;
        let separator = if fold { ' ' } else { '\n' };
        let newline_at = lines
            .get(index)
            .map(|(raw, offset)| offset + raw.len())
            .unwrap_or(0)
            .saturating_sub(1);
        text.push(separator);
        starts.push(newline_at);
        ends.push(newline_at + 1);
    }
    if !header.ends_with('-') && !header.ends_with('+') && !text.is_empty() && !text.ends_with('\n')
    {
        let newline_at = lines
            .last()
            .map(|(raw, offset)| offset + raw.len())
            .unwrap_or(0)
            .saturating_sub(1);
        text.push('\n');
        starts.push(newline_at);
        ends.push(newline_at + 1);
    }
    let source_start = starts.first().copied().unwrap_or(0);
    let source_end = ends.last().copied().unwrap_or(source_start);
    MappedText {
        text,
        starts,
        ends,
        source_start,
        source_end,
        host: host.to_owned(),
    }
}
