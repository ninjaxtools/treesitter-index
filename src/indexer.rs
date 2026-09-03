mod go;
mod java;
mod python;
mod rust;
mod typescript;

use std::{collections::BTreeMap, path::Path};

use regex::Regex;
use tree_sitter::Node;

const FIELD_TRUNCATE_THRESHOLD: usize = 8;
const LINE_WRAP_THRESHOLD: usize = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceLanguage {
    Python,
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
    Rust,
    Go,
    Java,
}

impl SourceLanguage {
    pub fn from_name(value: &str) -> Result<Self, String> {
        match value {
            "python" | "py" => Ok(Self::Python),
            "javascript" | "js" => Ok(Self::JavaScript),
            "jsx" => Ok(Self::Jsx),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "tsx" => Ok(Self::Tsx),
            "rust" | "rs" => Ok(Self::Rust),
            "go" => Ok(Self::Go),
            "java" => Ok(Self::Java),
            _ => Err(format!("unsupported language: {value}")),
        }
    }

    pub fn from_path(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("cannot infer language from {}", path.display()))?;
        match extension {
            "py" | "pyi" => Ok(Self::Python),
            "js" | "mjs" | "cjs" => Ok(Self::JavaScript),
            "jsx" => Ok(Self::Jsx),
            "ts" | "mts" | "cts" => Ok(Self::TypeScript),
            "tsx" => Ok(Self::Tsx),
            "rs" => Ok(Self::Rust),
            "go" => Ok(Self::Go),
            "java" => Ok(Self::Java),
            _ => Err(format!("unsupported file extension: .{extension}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::Jsx => "jsx",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Java => "java",
        }
    }

    pub fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript | Self::Jsx => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Section {
    Import,
    Module,
    Constant,
    Type,
    Trait,
    Impl,
    Function,
    Class,
    Macro,
}

#[derive(Clone, Copy)]
enum ChildStyle {
    Detailed,
    Brief,
}

struct Entry {
    section: Section,
    symbol_name: Option<String>,
    line_start: usize,
    line_end: usize,
    text: String,
    import_paths: Vec<Vec<String>>,
    import_keyword: Option<String>,
    attrs: Vec<String>,
    children: Vec<EntryChild>,
    child_style: ChildStyle,
}

struct EntryChild {
    text: String,
    range: Option<(usize, usize)>,
}

struct Extracted {
    entries: Vec<Entry>,
    module_doc: Option<(usize, usize)>,
    test_lines: Vec<usize>,
    import_separator: &'static str,
}

pub fn skeleton(language: SourceLanguage, root: Node<'_>, source: &[u8]) -> String {
    skeleton_matching(language, root, source, &[])
}

pub fn skeleton_matching(
    language: SourceLanguage,
    root: Node<'_>,
    source: &[u8],
    regexps: &[Regex],
) -> String {
    let mut extracted = extract(language, root, source);
    if !regexps.is_empty() {
        extracted.entries.retain(|entry| {
            matches!(entry.section, Section::Function | Section::Class)
                && entry
                    .symbol_name
                    .as_deref()
                    .is_some_and(|name| regexps.iter().any(|regexp| regexp.is_match(name)))
        });
        extracted.module_doc = None;
        extracted.test_lines.clear();
    }
    render_skeleton(&extracted)
}

fn extract(language: SourceLanguage, root: Node<'_>, source: &[u8]) -> Extracted {
    let mut entries = Vec::new();
    let mut test_lines = Vec::new();
    let module_doc = detect_module_doc(language, root, source);
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if is_attr(language, child) || is_doc_comment(language, child, source) {
            continue;
        }
        let attrs = collect_preceding_attrs(language, child, source);
        if is_test_node(language, child, &attrs) {
            test_lines.push(line_start(child));
            continue;
        }

        let mut extracted = match language {
            SourceLanguage::Python => python::extract(child, source, &attrs),
            SourceLanguage::JavaScript
            | SourceLanguage::Jsx
            | SourceLanguage::TypeScript
            | SourceLanguage::Tsx => typescript::extract(child, source, &attrs),
            SourceLanguage::Rust => rust::extract(child, source, &attrs),
            SourceLanguage::Go => go::extract(child, source, &attrs),
            SourceLanguage::Java => java::extract(child, source, &attrs),
        };
        if let Some(first) = extracted.first_mut()
            && let Some(doc_start) = doc_comment_start_line(language, child, source)
        {
            first.line_start = first.line_start.min(doc_start);
        }
        entries.append(&mut extracted);
    }

    Extracted {
        entries,
        module_doc,
        test_lines,
        import_separator: match language {
            SourceLanguage::Python | SourceLanguage::Java => ".",
            SourceLanguage::Rust => "::",
            _ => "/",
        },
    }
}

fn is_doc_comment(language: SourceLanguage, node: Node<'_>, source: &[u8]) -> bool {
    match language {
        SourceLanguage::Python => false,
        SourceLanguage::JavaScript
        | SourceLanguage::Jsx
        | SourceLanguage::TypeScript
        | SourceLanguage::Tsx => {
            node.kind() == "comment" && node_text(node, source).starts_with("/**")
        }
        SourceLanguage::Rust => {
            node.kind() == "line_comment"
                && node_text(node, source).starts_with("///")
                && !node_text(node, source).starts_with("////")
        }
        SourceLanguage::Go => node.kind() == "comment",
        SourceLanguage::Java => {
            node.kind() == "block_comment" && node_text(node, source).starts_with("/**")
        }
    }
}

fn is_module_doc(language: SourceLanguage, node: Node<'_>, source: &[u8]) -> bool {
    match language {
        SourceLanguage::Python => {
            node.kind() == "expression_statement"
                && node.child(0).is_some_and(|child| {
                    child.kind() == "string" && node_text(child, source).starts_with("\"\"\"")
                })
        }
        SourceLanguage::Rust => {
            node.kind() == "line_comment" && node_text(node, source).starts_with("//!")
        }
        _ => false,
    }
}

fn is_attr(language: SourceLanguage, node: Node<'_>) -> bool {
    language == SourceLanguage::Rust && node.kind() == "attribute_item"
}

fn is_test_node(language: SourceLanguage, node: Node<'_>, attrs: &[String]) -> bool {
    language == SourceLanguage::Rust
        && matches!(node.kind(), "mod_item" | "function_item")
        && attrs
            .iter()
            .any(|attr| attr == "#[test]" || attr == "#[cfg(test)]" || attr.ends_with("::test]"))
}

fn detect_module_doc(
    language: SourceLanguage,
    root: Node<'_>,
    source: &[u8],
) -> Option<(usize, usize)> {
    let mut range = None;
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if is_module_doc(language, child, source) {
            let end = child.end_position();
            let end_line = if end.column == 0 {
                end.row
            } else {
                end.row + 1
            };
            range = Some((
                range.map_or(line_start(child), |value: (usize, usize)| value.0),
                end_line,
            ));
        } else if !is_attr(language, child) && !child.is_extra() {
            break;
        }
    }
    range
}

fn collect_preceding_attrs(language: SourceLanguage, node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut attrs = Vec::new();
    let mut previous = node.prev_sibling();
    while let Some(node) = previous {
        if is_attr(language, node) {
            attrs.push(node_text(node, source).to_owned());
            previous = node.prev_sibling();
        } else {
            break;
        }
    }
    attrs.reverse();
    attrs
}

fn doc_comment_start_line(
    language: SourceLanguage,
    node: Node<'_>,
    source: &[u8],
) -> Option<usize> {
    let mut previous = node.prev_sibling();
    let mut earliest = None;
    while let Some(node) = previous {
        if is_attr(language, node) {
            previous = node.prev_sibling();
        } else if is_doc_comment(language, node, source) {
            earliest = Some(line_start(node));
            previous = node.prev_sibling();
        } else {
            break;
        }
    }
    earliest
}

fn render_skeleton(extracted: &Extracted) -> String {
    const SECTIONS: &[(Section, &str)] = &[
        (Section::Import, "imports:"),
        (Section::Module, "mod:"),
        (Section::Constant, "consts:"),
        (Section::Type, "types:"),
        (Section::Trait, "traits:"),
        (Section::Impl, "impls:"),
        (Section::Function, "fns:"),
        (Section::Class, "classes:"),
        (Section::Macro, "macros:"),
    ];
    let mut sections = Vec::new();

    if let Some((start, end)) = extracted.module_doc {
        sections.push(format!("module doc: {}", format_range(start, end)));
    }

    for &(section, header) in SECTIONS {
        let items: Vec<_> = extracted
            .entries
            .iter()
            .filter(|entry| entry.section == section)
            .collect();
        if items.is_empty() {
            continue;
        }

        if section == Section::Import {
            sections.push(render_imports(&items, extracted.import_separator));
        } else if section == Section::Module {
            let mut lines = vec![format!("{header} {}", entries_range(&items))];
            let names: Vec<_> = items.iter().map(|entry| entry.text.as_str()).collect();
            lines.extend(wrap_csv(&names, "  "));
            sections.push(lines.join("\n"));
        } else {
            let mut lines = vec![header.to_owned()];
            for entry in items {
                lines.extend(entry.attrs.iter().map(|attr| format!("  {attr}")));
                push_ranged(
                    &mut lines,
                    format!("  {}", entry.text),
                    (entry.line_start, entry.line_end),
                );
                render_children(&mut lines, entry);
            }
            sections.push(lines.join("\n"));
        }
    }

    if !extracted.test_lines.is_empty() {
        let start = *extracted.test_lines.iter().min().unwrap();
        let end = *extracted.test_lines.iter().max().unwrap();
        sections.push(format!("tests: {}", format_range(start, end)));
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!("{}\n", sections.join("\n\n"))
    }
}

fn render_imports(items: &[&Entry], separator: &str) -> String {
    let mut tries: BTreeMap<&str, ImportTrie> = BTreeMap::new();
    for entry in items {
        let trie = tries
            .entry(entry.import_keyword.as_deref().unwrap_or("import"))
            .or_default();
        for path in &entry.import_paths {
            trie.insert(path);
        }
    }

    let mut lines = vec![format!("imports: {}", entries_range(items))];
    for (keyword, trie) in tries {
        for value in trie.render(separator) {
            if keyword == "import" {
                lines.push(format!("  {value}"));
            } else {
                lines.push(format!("  {keyword}: {value}"));
            }
        }
    }
    lines.join("\n")
}

fn render_children(lines: &mut Vec<String>, entry: &Entry) {
    match entry.child_style {
        ChildStyle::Detailed => {
            for child in &entry.children {
                if let Some(range) = child.range {
                    push_ranged(lines, format!("    {}", child.text), range);
                } else {
                    lines.push(format!("    {}", child.text));
                }
            }
        }
        ChildStyle::Brief => {
            let has_truncation = entry.children.last().is_some_and(|child| {
                child.range.is_none() && child.text.ends_with(" more truncated]")
            });
            let end = entry.children.len() - usize::from(has_truncation);
            let values: Vec<_> = entry.children[..end]
                .iter()
                .map(|child| {
                    child.range.map_or_else(
                        || child.text.clone(),
                        |range| format!("{} {}", child.text, format_range(range.0, range.1)),
                    )
                })
                .collect();
            let refs: Vec<_> = values.iter().map(String::as_str).collect();
            lines.extend(wrap_csv(&refs, "    "));
            if has_truncation {
                lines.push(format!("    {}", entry.children.last().unwrap().text));
            }
        }
    }
}

fn entries_range(items: &[&Entry]) -> String {
    let start = items
        .iter()
        .map(|entry| entry.line_start)
        .min()
        .unwrap_or(1);
    let end = items
        .iter()
        .map(|entry| entry.line_end)
        .max()
        .unwrap_or(start);
    format_range(start, end)
}

fn push_ranged(lines: &mut Vec<String>, body: String, range: (usize, usize)) {
    let mut body_lines = body.lines();
    if let Some(first) = body_lines.next() {
        lines.push(format!("{first} {}", format_range(range.0, range.1)));
        let indent: String = body
            .chars()
            .take_while(|character| character.is_whitespace())
            .collect();
        lines.extend(body_lines.map(|line| format!("{indent}{line}")));
    }
}

fn wrap_csv(items: &[&str], indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = indent.to_owned();
    for (index, item) in items.iter().enumerate() {
        let addition = if index == 0 {
            (*item).to_owned()
        } else {
            format!(", {item}")
        };
        if index > 0 && current.len() + addition.len() > LINE_WRAP_THRESHOLD {
            lines.push(current);
            current = format!("{indent}{item}");
        } else {
            current.push_str(&addition);
        }
    }
    if !current.trim().is_empty() {
        lines.push(current);
    }
    lines
}

#[derive(Default)]
struct ImportTrie {
    children: BTreeMap<String, ImportTrie>,
    terminal: bool,
}

impl ImportTrie {
    fn insert(&mut self, path: &[String]) {
        let mut node = self;
        for segment in path {
            node = node.children.entry(segment.clone()).or_default();
        }
        node.terminal = true;
    }

    fn render(&self, separator: &str) -> Vec<String> {
        self.children
            .iter()
            .flat_map(|(segment, node)| node.render_node(segment, separator))
            .collect()
    }

    fn render_node(&self, segment: &str, separator: &str) -> Vec<String> {
        if self.children.is_empty() {
            return vec![segment.to_owned()];
        }
        let children = self.render(separator);
        if self.terminal {
            let mut output = vec![segment.to_owned()];
            output.extend(
                children
                    .into_iter()
                    .map(|child| format!("{segment}{separator}{child}")),
            );
            output
        } else if children.len() == 1 {
            vec![format!("{segment}{separator}{}", children[0])]
        } else {
            vec![format!("{segment}{separator}{{{}}}", children.join(", "))]
        }
    }
}

fn new_entry(section: Section, node: Node<'_>, text: String) -> Entry {
    Entry {
        section,
        symbol_name: None,
        line_start: line_start(node),
        line_end: line_end(node),
        text,
        import_paths: Vec::new(),
        import_keyword: None,
        attrs: Vec::new(),
        children: Vec::new(),
        child_style: ChildStyle::Detailed,
    }
}

fn new_symbol_entry(section: Section, node: Node<'_>, symbol_name: String, text: String) -> Entry {
    let mut entry = new_entry(section, node, text);
    entry.symbol_name = Some(symbol_name);
    entry
}

fn new_import_entry(node: Node<'_>, paths: Vec<Vec<String>>) -> Entry {
    let mut entry = new_entry(Section::Import, node, String::new());
    entry.import_paths = paths;
    entry
}

fn child(text: String) -> EntryChild {
    EntryChild { text, range: None }
}

fn ranged_child(text: String, node: Node<'_>) -> EntryChild {
    EntryChild {
        text,
        range: Some((line_start(node), line_end(node))),
    }
}

fn find_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn node_text<'source>(node: Node<'_>, source: &'source [u8]) -> &'source str {
    node.utf8_text(source).unwrap_or("")
}

fn line_start(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

fn line_end(node: Node<'_>) -> usize {
    node.end_position().row + 1
}

fn format_range(start: usize, end: usize) -> String {
    if start == end {
        format!("[{start}]")
    } else {
        format!("[{start}-{end}]")
    }
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut boundary = maximum.saturating_sub(11).min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let value = &value[..boundary];
    if value.contains('\n') {
        format!("{value}\n[truncated]")
    } else {
        format!("{value}[truncated]")
    }
}

fn truncate_child_count(children: &mut Vec<EntryChild>, total: usize) {
    if total > FIELD_TRUNCATE_THRESHOLD {
        children.push(child(format!(
            "[{} more truncated]",
            total - FIELD_TRUNCATE_THRESHOLD
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn index_matching(language: SourceLanguage, source: &str, patterns: &[&str]) -> String {
        index_matching_with_case(language, source, patterns, false)
    }

    fn index_matching_with_case(
        language: SourceLanguage,
        source: &str,
        patterns: &[&str],
        case_insensitive: bool,
    ) -> String {
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let patterns: Vec<_> = patterns
            .iter()
            .map(|pattern| {
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(case_insensitive)
                    .build()
                    .unwrap()
            })
            .collect();
        skeleton_matching(language, tree.root_node(), source.as_bytes(), &patterns)
    }

    #[test]
    fn infers_supported_extensions() {
        assert_eq!(
            SourceLanguage::from_path(Path::new("main.py")).unwrap(),
            SourceLanguage::Python
        );
        assert_eq!(
            SourceLanguage::from_path(Path::new("app.tsx")).unwrap(),
            SourceLanguage::Tsx
        );
        assert_eq!(
            SourceLanguage::from_path(Path::new("lib.rs")).unwrap(),
            SourceLanguage::Rust
        );
        assert!(SourceLanguage::from_path(Path::new("data.json")).is_err());
    }

    #[test]
    fn matches_python_and_typescript_classes_and_functions_by_bare_name() {
        let python = index_matching(
            SourceLanguage::Python,
            "import os\nVALUE = 1\n@decorated\nclass Repository:\n    def connect(self): pass\nasync def process_data(value): pass\ndef ignore_me(): pass\n",
            &["^Repo.*$", "^process_.ata$"],
        );
        assert!(python.contains("Repository"));
        assert!(python.contains("connect(self)"));
        assert!(python.contains("async process_data(value)"));
        assert!(!python.contains("import"));
        assert!(!python.contains("VALUE"));
        assert!(!python.contains("ignore_me"));

        let typescript = index_matching(
            SourceLanguage::TypeScript,
            "export interface ServiceShape {}\nexport const loader = () => 1;\nexport class Service { run(): void {} }\nexport function load(path: string): void {}\nclass Ignore {}\n",
            &["^Service$", "^lo.d$"],
        );
        assert!(typescript.contains("export Service"));
        assert!(typescript.contains("run(): void"));
        assert!(typescript.contains("export load(path: string): void"));
        assert!(!typescript.contains("ServiceShape"));
        assert!(!typescript.contains("loader"));
        assert!(!typescript.contains("Ignore"));
    }

    #[test]
    fn matches_rust_go_and_java_top_level_symbols() {
        let rust = index_matching(
            SourceLanguage::Rust,
            "struct LoadState;\nimpl LoadState { fn load(&self) {} }\npub fn load_data() {}\nfn ignore() {}\n",
            &["^load_.*$"],
        );
        assert!(rust.contains("pub load_data()"));
        assert!(!rust.contains("LoadState"));
        assert!(!rust.contains("ignore"));

        let go = index_matching(
            SourceLanguage::Go,
            "package main\ntype Service struct{}\nfunc (Service) Load() {}\nfunc LoadData() {}\nfunc Ignore() {}\n",
            &["^Load.*$"],
        );
        assert!(go.contains("LoadData()"));
        assert!(!go.contains("Service"));
        assert!(!go.contains("Ignore"));

        let java = index_matching(
            SourceLanguage::Java,
            "class Service { void run() {} }\ninterface ServiceShape {}\nrecord Point(int x, int y) {}\nclass Ignore {}\n",
            &["^Serv.*$", "^Point$"],
        );
        assert!(java.contains("class Service"));
        assert!(java.contains("void run()"));
        assert!(java.contains("record Point(int x, int y)"));
        assert!(!java.contains("ServiceShape"));
        assert!(!java.contains("Ignore"));
    }

    #[test]
    fn no_symbol_matches_produce_an_empty_skeleton() {
        assert!(index_matching(SourceLanguage::Rust, "fn present() {}\n", &["missing"]).is_empty());
    }

    #[test]
    fn symbol_regexps_are_case_sensitive_unless_requested() {
        let source = "fn LoadData() {}\n";
        assert!(index_matching(SourceLanguage::Rust, source, &["^loaddata$"]).is_empty());
        assert!(
            index_matching_with_case(SourceLanguage::Rust, source, &["^loaddata$"], true)
                .contains("LoadData")
        );
    }
}
