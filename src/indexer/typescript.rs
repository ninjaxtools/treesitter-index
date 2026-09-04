use tree_sitter::Node;

use super::{
    Entry, Section, compact_whitespace, find_child, new_entry, new_import_entry, new_symbol_entry,
    node_text, ranged_child, ranged_symbol_child, truncate, truncate_child_count,
};

const SIGNATURE_LIMIT: usize = 160;

pub(super) fn extract(node: Node<'_>, source: &[u8], attrs: &[String]) -> Vec<Entry> {
    match node.kind() {
        "import_statement" => extract_import(node, source).into_iter().collect(),
        "export_statement" => extract_export(node, source, attrs),
        "ambient_declaration" => extract_ambient(node, source, "", node, attrs),
        _ => extract_declaration(node, source, "", node, attrs),
    }
}

fn extract_import(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let text = compact_whitespace(node_text(node, source));
    let cleaned = text
        .strip_prefix("import")
        .unwrap_or(&text)
        .trim()
        .trim_end_matches(';')
        .trim();
    (!cleaned.is_empty()).then(|| new_import_entry(node, vec![segment_path(cleaned)]))
}

fn extract_export(node: Node<'_>, source: &[u8], attrs: &[String]) -> Vec<Entry> {
    if node.child_by_field_name("source").is_some() {
        let text = compact_whitespace(node_text(node, source));
        let cleaned = text
            .strip_prefix("export")
            .unwrap_or(&text)
            .trim()
            .trim_end_matches(';')
            .trim();
        if cleaned.is_empty() {
            return Vec::new();
        }
        let mut entry = new_import_entry(node, vec![segment_path(cleaned)]);
        entry.import_keyword = Some("export".to_owned());
        return vec![entry];
    }

    let Some(declaration) = declaration_child(node) else {
        return Vec::new();
    };
    let before = &source[node.start_byte()..declaration.start_byte()];
    let prefix = if String::from_utf8_lossy(before)
        .split_whitespace()
        .any(|part| part == "default")
    {
        "export default "
    } else {
        "export "
    };
    let outer_attrs = combined_attrs(attrs, node, source);

    if declaration.kind() == "ambient_declaration" {
        extract_ambient(declaration, source, prefix, node, &outer_attrs)
    } else {
        extract_declaration(declaration, source, prefix, node, &outer_attrs)
    }
}

fn extract_ambient(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Vec<Entry> {
    if let Some(declaration) = declaration_child(node) {
        let prefix = format!("{prefix}declare ");
        return extract_declaration(declaration, source, &prefix, range_node, attrs);
    }

    // `declare global` and `declare module.exports` do not wrap a declaration node.
    let header = signature_before(node, find_child(node, "statement_block"), source);
    if header.is_empty() {
        Vec::new()
    } else {
        vec![new_entry(
            Section::Module,
            range_node,
            truncate(&format!("{prefix}{header}"), SIGNATURE_LIMIT),
        )]
    }
}

fn extract_declaration(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Vec<Entry> {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" | "class" => {
            extract_class(node, source, prefix, range_node, attrs)
                .into_iter()
                .collect()
        }
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            extract_function(node, source, prefix, range_node, attrs)
                .into_iter()
                .collect()
        }
        "interface_declaration" => extract_interface(node, source, prefix, range_node, attrs)
            .into_iter()
            .collect(),
        "type_alias_declaration" => extract_type_alias(node, source, prefix, range_node, attrs)
            .into_iter()
            .collect(),
        "enum_declaration" => extract_enum(node, source, prefix, range_node, attrs)
            .into_iter()
            .collect(),
        "internal_module" | "module" => extract_module(node, source, prefix, range_node, attrs)
            .into_iter()
            .collect(),
        "lexical_declaration" | "variable_declaration" => {
            extract_variables(node, source, prefix, range_node, attrs)
        }
        "ambient_declaration" => extract_ambient(node, source, prefix, range_node, attrs),
        _ => Vec::new(),
    }
}

fn extract_class(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body")?;
    let mut text = String::from(prefix);
    if node.kind() == "abstract_class_declaration" {
        text.push_str("abstract ");
    }
    text.push_str(node_text(name, source));
    if let Some(parameters) = node.child_by_field_name("type_parameters") {
        text.push_str(node_text(parameters, source));
    }
    if let Some(heritage) = direct_child(node, "class_heritage") {
        text.push(' ');
        text.push_str(node_text(heritage, source));
    }

    let mut entry = new_symbol_entry(
        Section::Class,
        range_node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&text), SIGNATURE_LIMIT),
    );
    entry.attrs = combined_attrs(attrs, node, source);

    let mut field_count = 0;
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method_definition" | "method_signature" | "abstract_method_signature" => {
                let signature = member_signature(member, source, false);
                if !signature.is_empty()
                    && let Some(name) = member.child_by_field_name("name")
                {
                    entry.children.push(ranged_symbol_child(
                        signature,
                        member,
                        node_text(name, source).to_owned(),
                    ));
                }
            }
            "public_field_definition"
            | "property_definition"
            | "field_definition"
            | "property_signature"
            | "index_signature" => {
                field_count += 1;
                if field_count <= super::FIELD_TRUNCATE_THRESHOLD {
                    let signature = member_signature(member, source, true);
                    if !signature.is_empty() {
                        entry.children.push(ranged_child(signature, member));
                    }
                }
            }
            _ => {}
        }
    }
    truncate_child_count(&mut entry.children, field_count);
    Some(entry)
}

fn extract_function(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut text = String::from(prefix);
    let modifiers = &source[node.start_byte()..name.start_byte()];
    if String::from_utf8_lossy(modifiers)
        .split_whitespace()
        .any(|part| part == "async")
    {
        text.push_str("async ");
    }
    if node.kind() == "generator_function_declaration" {
        text.push('*');
    }
    text.push_str(node_text(name, source));
    append_field(&mut text, node, "type_parameters", source);
    if let Some(parameters) = node.child_by_field_name("parameters") {
        text.push_str(node_text(parameters, source));
    } else {
        text.push_str("()");
    }
    if let Some(return_type) = node.child_by_field_name("return_type") {
        append_return_type(&mut text, return_type, source);
    }

    let mut entry = new_symbol_entry(
        Section::Function,
        range_node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&text), SIGNATURE_LIMIT),
    );
    entry.attrs = combined_attrs(attrs, node, source);
    Some(entry)
}

fn extract_interface(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body")?;
    let mut text = format!("{prefix}interface {}", node_text(name, source));
    append_field(&mut text, node, "type_parameters", source);
    if let Some(extends) = direct_child(node, "extends_type_clause") {
        text.push(' ');
        text.push_str(node_text(extends, source));
    }

    let mut entry = new_entry(
        Section::Type,
        range_node,
        truncate(&compact_whitespace(&text), SIGNATURE_LIMIT),
    );
    entry.attrs = combined_attrs(attrs, node, source);

    let mut total = 0;
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if matches!(
            member.kind(),
            "property_signature"
                | "method_signature"
                | "call_signature"
                | "construct_signature"
                | "index_signature"
        ) {
            total += 1;
            if total <= super::FIELD_TRUNCATE_THRESHOLD {
                let signature = member_signature(member, source, false);
                if !signature.is_empty() {
                    entry.children.push(ranged_child(signature, member));
                }
            }
        }
    }
    truncate_child_count(&mut entry.children, total);
    Some(entry)
}

fn extract_type_alias(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut text = format!("{prefix}type {}", node_text(name, source));
    append_field(&mut text, node, "type_parameters", source);
    if let Some(value) = node.child_by_field_name("value") {
        let value = compact_whitespace(node_text(value, source));
        text.push_str(" = ");
        text.push_str(&truncate(&value, 80));
    }
    let mut entry = new_entry(Section::Type, range_node, text);
    entry.attrs = combined_attrs(attrs, node, source);
    Some(entry)
}

fn extract_enum(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut entry = new_entry(
        Section::Type,
        range_node,
        format!("{prefix}enum {}", node_text(name, source)),
    );
    entry.attrs = combined_attrs(attrs, node, source);
    Some(entry)
}

fn extract_module(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Option<Entry> {
    node.child_by_field_name("name")?;
    let header = signature_before(node, node.child_by_field_name("body"), source);
    let mut entry = new_entry(
        Section::Module,
        range_node,
        truncate(&format!("{prefix}{header}"), SIGNATURE_LIMIT),
    );
    entry.attrs = combined_attrs(attrs, node, source);
    Some(entry)
}

fn extract_variables(
    node: Node<'_>,
    source: &[u8],
    prefix: &str,
    range_node: Node<'_>,
    attrs: &[String],
) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut cursor = node.walk();
    for declarator in node
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "variable_declarator")
    {
        let Some(name) = declarator.child_by_field_name("name") else {
            continue;
        };
        let mut text = format!("{prefix}{}", node_text(name, source));
        if let Some(type_node) = declarator.child_by_field_name("type") {
            append_return_type(&mut text, type_node, source);
        }
        if let Some(value) = declarator.child_by_field_name("value") {
            text.push_str(" = ");
            if is_function_value(value) {
                text.push_str(&signature_before(
                    value,
                    value.child_by_field_name("body"),
                    source,
                ));
            } else {
                let value = compact_whitespace(node_text(value, source));
                text.push_str(&truncate(&value, 60));
            }
        }

        let mut entry = new_entry(
            Section::Constant,
            range_node,
            truncate(&compact_whitespace(&text), SIGNATURE_LIMIT),
        );
        if entries.is_empty() {
            entry.attrs = combined_attrs(attrs, node, source);
        }
        entries.push(entry);
    }
    entries
}

fn declaration_child(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("declaration").or_else(|| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|child| {
            matches!(
                child.kind(),
                "class_declaration"
                    | "abstract_class_declaration"
                    | "function_declaration"
                    | "generator_function_declaration"
                    | "function_signature"
                    | "interface_declaration"
                    | "type_alias_declaration"
                    | "enum_declaration"
                    | "internal_module"
                    | "module"
                    | "lexical_declaration"
                    | "variable_declaration"
                    | "ambient_declaration"
            )
        })
    })
}

fn direct_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn append_field(text: &mut String, node: Node<'_>, field: &str, source: &[u8]) {
    if let Some(value) = node.child_by_field_name(field) {
        text.push_str(node_text(value, source));
    }
}

fn append_return_type(text: &mut String, node: Node<'_>, source: &[u8]) {
    let value = node_text(node, source);
    if !value.starts_with(':') {
        text.push_str(": ");
    }
    text.push_str(value);
}

fn member_signature(node: Node<'_>, source: &[u8], omit_value: bool) -> String {
    let end = if omit_value {
        node.child_by_field_name("value")
    } else {
        node.child_by_field_name("body")
    };
    let signature = signature_before(node, end, source);
    let signature = if omit_value {
        signature.trim_end_matches([' ', '='])
    } else {
        &signature
    };
    truncate(signature, SIGNATURE_LIMIT)
}

fn signature_before(node: Node<'_>, end: Option<Node<'_>>, source: &[u8]) -> String {
    let end_byte = end.map_or(node.end_byte(), |end| end.start_byte());
    let text = &source[node.start_byte()..end_byte];
    compact_whitespace(
        String::from_utf8_lossy(text)
            .trim()
            .trim_end_matches([';', ','])
            .trim(),
    )
}

fn is_function_value(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "arrow_function" | "function_expression" | "generator_function"
    )
}

fn segment_path(value: &str) -> Vec<String> {
    let segments: Vec<_> = value
        .split('/')
        .map(|part| part.trim().to_owned())
        .collect();
    if segments.is_empty() {
        vec![value.to_owned()]
    } else {
        segments
    }
}

fn combined_attrs(attrs: &[String], node: Node<'_>, source: &[u8]) -> Vec<String> {
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

    fn index(source: &str, language: SourceLanguage) -> String {
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        skeleton(language, tree.root_node(), source.as_bytes())
    }

    #[test]
    fn extracts_typescript_declarations_and_elides_bodies() {
        let output = index(
            "import { A } from './api/client';\n\
             export interface Config { port: number; run(x: string): void; }\n\
             export type ID = string | number;\n\
             export enum Direction { Up, Down }\n\
             export const handler = (value: string): number => { return value.length; };\n\
             export class Service { private value: string = 'x'; run(): void { work(); } }\n\
             export function load(path: string): Promise<void> { return open(path); }",
            SourceLanguage::TypeScript,
        );

        assert!(output.contains("{ A } from './api/client'"));
        assert!(output.contains("export interface Config"));
        assert!(output.contains("run(x: string): void"));
        assert!(output.contains("export handler = (value: string): number =>"));
        assert!(output.contains("private value: string"));
        assert!(output.contains("export load(path: string): Promise<void>"));
        assert!(!output.contains("return value.length"));
        assert!(!output.contains("work();"));
    }

    #[test]
    fn handles_javascript_signatures_without_type_nodes() {
        let output = index(
            "export const render = async (value) => value;\n\
             export function* values(items) { yield* items; }\n\
             export class View { field = 1; draw(target) { target.paint(); } }",
            SourceLanguage::JavaScript,
        );

        assert!(output.contains("export render = async (value) =>"));
        assert!(output.contains("export *values(items)"));
        assert!(output.contains("draw(target)"));
        assert!(!output.contains("yield* items"));
        assert!(!output.contains("target.paint"));
    }

    #[test]
    fn extracts_reexports_abstract_classes_and_modules() {
        let output = index(
            "export { Client } from './api/client';\n\
             export namespace API { export const version = 1; }\n\
             @sealed\nexport abstract class Base { abstract run(): void; }\n\
             declare module 'virtual' { export function open(): void; }",
            SourceLanguage::TypeScript,
        );

        assert!(output.contains("export: { Client } from './api/client'"));
        assert!(output.contains("export namespace API"));
        assert!(output.contains("@sealed"));
        assert!(output.contains("export abstract Base"));
        assert!(output.contains("abstract run(): void"));
        assert!(output.contains("declare module 'virtual'"));
    }
}
