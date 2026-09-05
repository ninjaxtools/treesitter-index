use tree_sitter::Node;

use super::{
    Entry, Section, compact_whitespace, find_child, new_entry, new_import_entry, new_symbol_entry,
    new_symbols_entry, node_text, ranged_child, ranged_symbol_child, truncate,
    truncate_child_count,
};

const TYPE_LIMIT: usize = 60;

pub(super) fn extract(node: Node<'_>, source: &[u8], _attrs: &[String]) -> Vec<Entry> {
    match node.kind() {
        "package_clause" => extract_package(node, source).into_iter().collect(),
        "import_declaration" => extract_import(node, source).into_iter().collect(),
        "const_declaration" => extract_values(node, source, false),
        "var_declaration" => extract_values(node, source, true),
        "type_declaration" => extract_types(node, source),
        "function_declaration" => extract_function(node, source).into_iter().collect(),
        "method_declaration" => extract_method(node, source).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn extract_package(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = find_child(node, "package_identifier")?;
    Some(new_symbol_entry(
        Section::Module,
        node,
        node_text(name, source).to_owned(),
        node_text(name, source).to_owned(),
    ))
}

fn extract_import(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let mut specs = Vec::new();
    collect_specs(node, "import_spec", "import_spec_list", &mut specs);

    let paths = specs
        .into_iter()
        .filter_map(|spec| {
            let path = spec.child_by_field_name("path")?;
            let value = strip_quotes(node_text(path, source).trim());
            if value.is_empty() {
                return None;
            }
            Some(value.split('/').map(str::to_owned).collect())
        })
        .collect::<Vec<_>>();

    (!paths.is_empty()).then(|| new_import_entry(node, paths))
}

fn strip_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && matches!(bytes[0], b'\'' | b'"' | b'`')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn extract_values(node: Node<'_>, source: &[u8], is_var: bool) -> Vec<Entry> {
    let (spec_kind, list_kind) = if is_var {
        ("var_spec", "var_spec_list")
    } else {
        ("const_spec", "const_spec_list")
    };
    let mut specs = Vec::new();
    collect_specs(node, spec_kind, list_kind, &mut specs);

    specs
        .into_iter()
        .filter_map(|spec| {
            let names = field_texts(spec, "name", source);
            if names.is_empty() {
                return None;
            }

            let mut text = if is_var {
                format!("var {}", names.join(", "))
            } else {
                names.join(", ")
            };
            if let Some(value_type) = spec.child_by_field_name("type") {
                text.push(' ');
                text.push_str(node_text(value_type, source));
            }
            Some(new_symbols_entry(
                Section::Constant,
                spec,
                names,
                compact_whitespace(&text),
            ))
        })
        .collect()
}

fn extract_types(node: Node<'_>, source: &[u8]) -> Vec<Entry> {
    let mut declarations = Vec::new();
    collect_specs(node, "type_spec", "type_spec_list", &mut declarations);
    collect_specs(node, "type_alias", "type_spec_list", &mut declarations);
    declarations.sort_by_key(Node::start_byte);

    declarations
        .into_iter()
        .filter_map(|declaration| match declaration.kind() {
            "type_alias" => extract_type_alias(declaration, source),
            "type_spec" => extract_type_spec(declaration, source),
            _ => None,
        })
        .collect()
}

fn extract_type_alias(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let value = node.child_by_field_name("type")?;
    Some(new_symbol_entry(
        Section::Type,
        node,
        node_text(name, source).to_owned(),
        compact_whitespace(&format!(
            "type {} = {}",
            node_text(name, source),
            node_text(value, source)
        )),
    ))
}

fn extract_type_spec(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let value = node.child_by_field_name("type")?;
    let symbol_name = node_text(name, source).to_owned();
    let mut name = symbol_name.clone();
    if let Some(parameters) = node.child_by_field_name("type_parameters") {
        name.push_str(node_text(parameters, source));
    }

    let mut entry = match value.kind() {
        "struct_type" => Some(extract_struct(node, value, source, &name)),
        "interface_type" => Some(extract_interface(node, value, source, &name)),
        _ => {
            let value = compact_whitespace(node_text(value, source));
            Some(new_entry(
                Section::Type,
                node,
                format!("{name} {}", truncate(&value, TYPE_LIMIT)),
            ))
        }
    }?;
    entry.symbol_names.push(symbol_name);
    Some(entry)
}

fn extract_struct(
    declaration: Node<'_>,
    struct_type: Node<'_>,
    source: &[u8],
    name: &str,
) -> Entry {
    let mut entry = new_entry(Section::Type, declaration, format!("struct {name}"));
    let Some(body) = find_child(struct_type, "field_declaration_list") else {
        return entry;
    };

    let mut total = 0;
    let mut cursor = body.walk();
    for field in body
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "field_declaration")
    {
        total += 1;
        if total > super::FIELD_TRUNCATE_THRESHOLD {
            continue;
        }

        let names = field_texts(field, "name", source);
        let field_type = field
            .child_by_field_name("type")
            .map(|node| node_text(node, source))
            .unwrap_or("_");
        let text = if names.is_empty() {
            if find_child(field, "*").is_some() {
                format!("*{field_type}")
            } else {
                field_type.to_owned()
            }
        } else {
            format!("{} {field_type}", names.join(", "))
        };
        entry
            .children
            .push(ranged_child(compact_whitespace(&text), field));
    }
    truncate_child_count(&mut entry.children, total);
    entry
}

fn extract_interface(
    declaration: Node<'_>,
    interface_type: Node<'_>,
    source: &[u8],
    name: &str,
) -> Entry {
    let mut entry = new_entry(Section::Type, declaration, format!("interface {name}"));
    let mut cursor = interface_type.walk();
    for member in interface_type.named_children(&mut cursor) {
        let text = match member.kind() {
            "method_elem" => signature(member, source),
            "type_elem" => {
                let text = compact_whitespace(node_text(member, source));
                Some(truncate(&text, TYPE_LIMIT))
            }
            _ => None,
        };
        if let Some(text) = text {
            let child = if let Some(name) = member.child_by_field_name("name") {
                ranged_symbol_child(text, member, node_text(name, source).to_owned())
            } else {
                ranged_child(text, member)
            };
            entry.children.push(child);
        }
    }
    entry
}

fn extract_function(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let text = signature(node, source)?;
    Some(new_symbol_entry(
        Section::Function,
        node,
        node_text(name, source).to_owned(),
        text,
    ))
}

fn extract_method(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let receiver = node
        .child_by_field_name("receiver")
        .map(|receiver| node_text(receiver, source));
    let signature = signature(node, source)?;
    let text = receiver.map_or(signature.clone(), |receiver| {
        format!("{receiver} {signature}")
    });
    Some(new_symbol_entry(
        Section::Impl,
        node,
        node_text(name, source).to_owned(),
        compact_whitespace(&text),
    ))
}

fn signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let mut text = node_text(name, source).to_owned();
    if let Some(type_parameters) = node.child_by_field_name("type_parameters") {
        text.push_str(node_text(type_parameters, source));
    }
    if let Some(parameters) = node.child_by_field_name("parameters") {
        text.push_str(node_text(parameters, source));
    } else {
        text.push_str("()");
    }
    if let Some(result) = node.child_by_field_name("result") {
        text.push(' ');
        text.push_str(node_text(result, source));
    }
    Some(compact_whitespace(&text))
}

fn field_texts(node: Node<'_>, field: &str, source: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    node.children_by_field_name(field, &mut cursor)
        .filter(|child| child.is_named())
        .map(|child| node_text(child, source).to_owned())
        .collect()
}

fn collect_specs<'tree>(
    node: Node<'tree>,
    spec_kind: &str,
    list_kind: &str,
    specs: &mut Vec<Node<'tree>>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == spec_kind {
            specs.push(child);
        } else if child.kind() == list_kind {
            let mut list_cursor = child.walk();
            specs.extend(
                child
                    .named_children(&mut list_cursor)
                    .filter(|item| item.kind() == spec_kind),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use regex::Regex;

    use super::super::{SourceLanguage, skeleton_matching};

    fn index(source: &str) -> String {
        index_matching(source, &[])
    }

    fn index_matching(source: &str, patterns: &[&str]) -> String {
        let language = SourceLanguage::Go;
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let regexps: Vec<_> = patterns
            .iter()
            .map(|pattern| Regex::new(pattern).unwrap())
            .collect();
        skeleton_matching(language, tree.root_node(), source.as_bytes(), &regexps)
    }

    #[test]
    fn extracts_go_declarations_and_elides_function_bodies() {
        let output = index(
            "package api\n\
             import (\n\
               \"fmt\"\n\
               alias \"example.com/project/pkg\"\n\
               . \"example.com/project/dot\"\n\
               _ \"example.com/project/blank\"\n\
             )\n\
             const ( A = 1; B string = \"b\" )\n\
             var Global int\n\
             type Point struct { X, Y int; Name string }\n\
             type Reader interface { Read(p []byte) (int, error); ~int | ~string }\n\
             type Alias = map[string]int\n\
             func Load[T any](value T) error { return nil }\n\
             func (p *Point) Distance() float64 { return 0 }",
        );

        for expected in [
            "mod: [1]",
            "  api",
            "fmt",
            "example.com/project/{blank, dot, pkg}",
            "A",
            "B string",
            "var Global int",
            "struct Point",
            "X, Y int",
            "interface Reader",
            "Read(p []byte) (int, error)",
            "type Alias = map[string]int",
            "Load[T any](value T) error",
            "(p *Point) Distance() float64",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
        assert!(!output.contains("return nil"));
        assert!(!output.contains("return 0"));
    }

    #[test]
    fn ranges_members_and_truncates_struct_fields() {
        let output = index(
            "package p\n\
             type Many struct {\n\
               A int\nB int\nC int\nD int\nE int\nF int\nG int\nH int\nI int\n\
             }\n\
             type Store interface {\n\
               Get(key string) (string, error)\n\
             }",
        );

        assert!(output.contains("A int [3]"));
        assert!(output.contains("[1 more truncated]"));
        assert!(output.contains("Get(key string) (string, error) [14]"));
    }

    #[test]
    fn preserves_embedded_pointer_fields_and_field_truncation() {
        let output = index(
            "package p\n\
             type Record struct {\n\
               *Base\n\
               *pkg.Remote `json:\"remote\"`\n\
               Plain\n\
               pkg.Value\n\
               *Generic[int]\n\
               X, Y *Base\n\
               _ int\n\
               Named string `json:\"name\"`\n\
               *Hidden\n\
             }",
        );
        for expected in [
            "*Base [3]",
            "*pkg.Remote [4]",
            "Plain [5]",
            "pkg.Value [6]",
            "*Generic[int] [7]",
            "X, Y *Base [8]",
            "_ int [9]",
            "Named string [10]",
            "[1 more truncated]",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
        assert!(!output.contains("_ Base"));
        assert!(!output.contains("_ Plain"));
        assert!(!output.contains("json:"));
        assert!(!output.contains("Hidden"));
    }

    #[test]
    fn filters_interface_methods_by_bare_name() {
        let source = "package p\n\
            type Store interface {\n\
                Get(\n\
                    key string,\n\
                ) (string, error)\n\
                Put(key, value string) error\n\
                Base\n\
            }";
        for pattern in ["^Get$", "^(Store|Get)$"] {
            let output = index_matching(source, &[pattern]);
            assert!(output.contains("interface Store [2-8]"), "{output}");
            assert!(
                output.contains("Get( key string, ) (string, error) [3-5]"),
                "{output}"
            );
            assert!(!output.contains("Put("));
            assert!(!output.contains("Base"));
        }
        let parent = index_matching(source, &["^Store$"]);
        assert!(parent.contains("interface Store [2-8]"), "{parent}");
        assert!(!parent.contains("Get("));
        assert!(!parent.contains("Put("));
        assert!(!parent.contains("Base"));
        assert!(index_matching(source, &["^key$"]).is_empty());
    }
}
