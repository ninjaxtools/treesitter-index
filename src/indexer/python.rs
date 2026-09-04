use tree_sitter::Node;

use super::{
    Entry, Section, compact_whitespace, find_child, new_import_entry, new_symbol_entry, node_text,
    ranged_child, ranged_symbol_child, symbol_child, truncate, truncate_child_count,
};

const SIGNATURE_LIMIT: usize = 160;
const VALUE_LIMIT: usize = 60;

pub(super) fn extract(node: Node<'_>, source: &[u8], attrs: &[String]) -> Vec<Entry> {
    match node.kind() {
        "import_statement" | "import_from_statement" | "future_import_statement" => {
            extract_import(node, source).into_iter().collect()
        }
        "class_definition" => extract_class(node, source, node, attrs)
            .into_iter()
            .collect(),
        "function_definition" => extract_function(node, source, node, attrs)
            .into_iter()
            .collect(),
        "decorated_definition" => extract_decorated(node, source, attrs).into_iter().collect(),
        "expression_statement" => extract_constant_statement(node, source)
            .into_iter()
            .collect(),
        "type_alias_statement" => extract_type_alias(node, source, node).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn extract_import(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let paths = match node.kind() {
        "import_statement" => import_names(node)
            .into_iter()
            .map(|name| segmented_path(node_text(name, source)))
            .filter(|path| !path.is_empty())
            .collect(),
        "future_import_statement" => from_import_paths(node, source, vec!["__future__".to_owned()]),
        "import_from_statement" => {
            let module = node.child_by_field_name("module_name")?;
            from_import_paths(node, source, segmented_path(node_text(module, source)))
        }
        _ => Vec::new(),
    };

    (!paths.is_empty()).then(|| new_import_entry(node, paths))
}

fn import_names(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| matches!(child.kind(), "dotted_name" | "aliased_import"))
        .collect()
}

fn from_import_paths(node: Node<'_>, source: &[u8], base: Vec<String>) -> Vec<Vec<String>> {
    let module = node.child_by_field_name("module_name");
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| {
            Some(*child) != module
                && matches!(
                    child.kind(),
                    "dotted_name" | "aliased_import" | "wildcard_import"
                )
        })
        .map(|name| {
            let mut path = base.clone();
            path.push(compact_whitespace(node_text(name, source)));
            path
        })
        .filter(|path| path.last().is_some_and(|part| !part.is_empty()))
        .collect()
}

fn segmented_path(value: &str) -> Vec<String> {
    value
        .split('.')
        .map(|part| compact_whitespace(part.trim()))
        .filter(|part| !part.is_empty())
        .collect()
}

fn extract_decorated(node: Node<'_>, source: &[u8], attrs: &[String]) -> Option<Entry> {
    let definition = node
        .child_by_field_name("definition")
        .or_else(|| find_child(node, "class_definition"))
        .or_else(|| find_child(node, "function_definition"))?;
    let attrs = decorators(attrs, node, source);

    match definition.kind() {
        "class_definition" => extract_class(definition, source, node, &attrs),
        "function_definition" => extract_function(definition, source, node, &attrs),
        _ => None,
    }
}

fn extract_class(
    node: Node<'_>,
    source: &[u8],
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body")?;
    let mut signature = node_text(name, source).to_owned();
    append_field(&mut signature, node, "type_parameters", source);
    append_field(&mut signature, node, "superclasses", source);

    let mut entry = new_symbol_entry(
        Section::Class,
        range_node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&signature), SIGNATURE_LIMIT),
    );
    entry.attrs = attrs.to_vec();

    let mut field_count = 0;
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "function_definition" => push_method(&mut entry, member, member, source, &[]),
            "decorated_definition" => {
                if let Some(function) = member
                    .child_by_field_name("definition")
                    .filter(|definition| definition.kind() == "function_definition")
                    .or_else(|| find_child(member, "function_definition"))
                {
                    let method_attrs = decorators(&[], member, source);
                    push_method(&mut entry, function, function, source, &method_attrs);
                }
            }
            "expression_statement" => {
                if let Some(assignment) = assignment_child(member)
                    && let Some(text) = assignment_text(assignment, source, true, false)
                {
                    field_count += 1;
                    if field_count <= super::FIELD_TRUNCATE_THRESHOLD {
                        entry.children.push(ranged_child(text, assignment));
                    }
                }
            }
            "type_alias_statement" => {
                field_count += 1;
                if field_count <= super::FIELD_TRUNCATE_THRESHOLD
                    && let Some(text) = type_alias_text(member, source)
                {
                    entry.children.push(ranged_child(text, member));
                }
            }
            _ => {}
        }
    }
    truncate_child_count(&mut entry.children, field_count);
    Some(entry)
}

fn push_method(
    entry: &mut Entry,
    function: Node<'_>,
    range_node: Node<'_>,
    source: &[u8],
    attrs: &[String],
) {
    let Some(name) = function.child_by_field_name("name") else {
        return;
    };
    let symbol_name = node_text(name, source).to_owned();
    entry.children.extend(
        attrs
            .iter()
            .cloned()
            .map(|attr| symbol_child(attr, symbol_name.clone())),
    );
    if let Some(signature) = function_signature(function, source) {
        entry
            .children
            .push(ranged_symbol_child(signature, range_node, symbol_name));
    }
}

fn extract_function(
    node: Node<'_>,
    source: &[u8],
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut entry = new_symbol_entry(
        Section::Function,
        range_node,
        node_text(name, source).to_owned(),
        function_signature(node, source)?,
    );
    entry.attrs = attrs.to_vec();
    Some(entry)
}

fn function_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let mut signature = String::new();
    let prefix = &source[node.start_byte()..name.start_byte()];
    if String::from_utf8_lossy(prefix)
        .split_whitespace()
        .any(|part| part == "async")
    {
        signature.push_str("async ");
    }
    signature.push_str(node_text(name, source));
    append_field(&mut signature, node, "type_parameters", source);
    if let Some(parameters) = node.child_by_field_name("parameters") {
        signature.push_str(node_text(parameters, source));
    } else {
        signature.push_str("()");
    }
    if let Some(return_type) = node.child_by_field_name("return_type") {
        signature.push_str(" -> ");
        signature.push_str(node_text(return_type, source));
    }
    Some(truncate(&compact_whitespace(&signature), SIGNATURE_LIMIT))
}

fn extract_constant_statement(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let assignment = assignment_child(node)?;
    let name = assignment.child_by_field_name("left")?;
    let text = assignment_text(assignment, source, false, true)?;
    Some(new_symbol_entry(
        Section::Constant,
        assignment,
        node_text(name, source).to_owned(),
        text,
    ))
}

fn assignment_child(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "assignment" {
        Some(node)
    } else {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .find(|child| child.kind() == "assignment")
    }
}

fn assignment_text(
    node: Node<'_>,
    source: &[u8],
    include_type: bool,
    constants_only: bool,
) -> Option<String> {
    let left = node.child_by_field_name("left")?;
    let name = node_text(left, source);
    if left.kind() != "identifier"
        || (constants_only
            && (name.is_empty()
                || !name
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_uppercase())))
    {
        return None;
    }

    let mut text = name.to_owned();
    if include_type && let Some(type_node) = node.child_by_field_name("type") {
        text.push_str(": ");
        text.push_str(node_text(type_node, source));
    }
    if let Some(value) = node.child_by_field_name("right") {
        text.push_str(" = ");
        text.push_str(&truncate(
            &compact_whitespace(node_text(value, source)),
            VALUE_LIMIT,
        ));
    }
    Some(truncate(&compact_whitespace(&text), SIGNATURE_LIMIT))
}

fn extract_type_alias(node: Node<'_>, source: &[u8], range_node: Node<'_>) -> Option<Entry> {
    let left = node.child_by_field_name("left")?;
    Some(new_symbol_entry(
        Section::Type,
        range_node,
        first_identifier(left, source)?,
        type_alias_text(node, source)?,
    ))
}

fn first_identifier(node: Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(node_text(node, source).to_owned());
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| first_identifier(child, source))
}

fn type_alias_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    let left = node.child_by_field_name("left")?;
    let right = node.child_by_field_name("right")?;
    let right = truncate(&compact_whitespace(node_text(right, source)), VALUE_LIMIT);
    Some(truncate(
        &compact_whitespace(&format!("type {} = {right}", node_text(left, source))),
        SIGNATURE_LIMIT,
    ))
}

fn append_field(text: &mut String, node: Node<'_>, field: &str, source: &[u8]) {
    if let Some(value) = node.child_by_field_name(field) {
        text.push_str(node_text(value, source));
    }
}

fn decorators(attrs: &[String], node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut result = attrs.to_vec();
    let mut cursor = node.walk();
    for decorator in node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "decorator")
    {
        let decorator = compact_whitespace(node_text(decorator, source));
        if !result.contains(&decorator) {
            result.push(decorator);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use super::super::{SourceLanguage, skeleton};

    fn index(source: &str) -> String {
        let mut parser = Parser::new();
        parser
            .set_language(&SourceLanguage::Python.grammar())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        skeleton(SourceLanguage::Python, tree.root_node(), source.as_bytes())
    }

    #[test]
    fn extracts_python_sections_and_elides_bodies() {
        let output = index(
            "import os.path, collections.abc as abc\n\
             from typing import Optional, Iterable as Items\n\
             MAX_RETRIES: int = 3\n\
             type Result[T] = tuple[T, Exception | None]\n\
             @registered('worker')\n\
             async def process(data: list[int]) -> dict[str, int]:\n\
                 \"\"\"Process some data.\"\"\"\n\
                 return {'size': len(data)}\n",
        );

        assert!(output.contains("os.path"));
        assert!(output.contains("collections.abc as abc"));
        assert!(output.contains("typing.{Iterable as Items, Optional}"));
        assert!(output.contains("MAX_RETRIES = 3"));
        assert!(output.contains("type Result[T] = tuple[T, Exception | None]"));
        assert!(output.contains("@registered('worker')"));
        assert!(output.contains("async process(data: list[int]) -> dict[str, int]"));
        assert!(!output.contains("return {'size'"));
    }

    #[test]
    fn extracts_class_bases_methods_and_fields_with_ranges() {
        let output = index(
            "class Repo[T](Base, Protocol):\n    url: str = 'local'\n    @classmethod\n    async def connect(cls, url: str) -> None:\n        \"\"\"Connect to the repository.\"\"\"\n        await open(url)\n",
        );

        assert!(output.contains("Repo[T](Base, Protocol)"));
        assert!(output.contains("url: str = 'local' [2]"));
        assert!(output.contains("@classmethod"));
        assert!(output.contains("async connect(cls, url: str) -> None [4-6]"));
        assert!(!output.contains("await open"));
    }
}
