use tree_sitter::Node;

use super::{
    ChildStyle, Entry, Section, child, compact_whitespace, find_child, new_import_entry,
    new_symbol_entry, node_text, ranged_child, ranged_symbol_child, truncate, truncate_child_count,
};

const SIGNATURE_LIMIT: usize = 160;

pub(super) fn extract(node: Node<'_>, source: &[u8], _attrs: &[String]) -> Vec<Entry> {
    let entry = match node.kind() {
        "package_declaration" => extract_package(node, source),
        "import_declaration" => extract_import(node, source),
        "class_declaration" => extract_class(node, source),
        "interface_declaration" => extract_interface(node, source),
        "enum_declaration" => extract_enum(node, source),
        "record_declaration" => extract_record(node, source),
        "annotation_type_declaration" => extract_annotation_type(node, source),
        _ => None,
    };
    entry.into_iter().collect()
}

fn extract_package(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let text = compact_whitespace(node_text(node, source));
    let package = text
        .strip_prefix("package")
        .unwrap_or(&text)
        .trim()
        .trim_end_matches(';')
        .trim();
    (!package.is_empty()).then(|| {
        new_symbol_entry(
            Section::Module,
            node,
            package.to_owned(),
            package.to_owned(),
        )
    })
}

fn extract_import(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let text = compact_whitespace(node_text(node, source));
    let import = text
        .strip_prefix("import")
        .unwrap_or(&text)
        .trim()
        .trim_end_matches(';')
        .trim();
    if import.is_empty() {
        return None;
    }

    let path: Vec<_> = import
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    (!path.is_empty()).then(|| new_import_entry(node, vec![path]))
}

fn extract_class(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut label = declaration_prefix(node, source);
    label.push_str("class ");
    label.push_str(node_text(name, source));
    append_field(&mut label, node, "type_parameters", source, false);
    append_field(&mut label, node, "superclass", source, true);
    append_field(&mut label, node, "interfaces", source, true);

    let mut entry = new_symbol_entry(
        Section::Class,
        node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&label), SIGNATURE_LIMIT),
    );
    if let Some(body) = node.child_by_field_name("body") {
        entry.children = extract_members(body, source, false);
    }
    Some(entry)
}

fn extract_interface(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut label = declaration_prefix(node, source);
    label.push_str("interface ");
    label.push_str(node_text(name, source));
    append_field(&mut label, node, "type_parameters", source, false);
    if let Some(extends) = find_child(node, "extends_interfaces") {
        label.push(' ');
        label.push_str(node_text(extends, source));
    }

    let mut entry = new_symbol_entry(
        Section::Trait,
        node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&label), SIGNATURE_LIMIT),
    );
    if let Some(body) = node.child_by_field_name("body") {
        entry.children = extract_members(body, source, true);
    }
    Some(entry)
}

fn extract_enum(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut label = declaration_prefix(node, source);
    label.push_str("enum ");
    label.push_str(node_text(name, source));
    append_field(&mut label, node, "interfaces", source, true);

    let mut entry = new_symbol_entry(
        Section::Type,
        node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&label), SIGNATURE_LIMIT),
    );
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for variant in body
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "enum_constant")
        {
            if let Some(name) = variant.child_by_field_name("name") {
                entry
                    .children
                    .push(child(node_text(name, source).to_owned()));
            }
        }
        entry.child_style = ChildStyle::Brief;
    }
    Some(entry)
}

fn extract_record(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let parameters = node.child_by_field_name("parameters")?;
    let mut label = declaration_prefix(node, source);
    label.push_str("record ");
    label.push_str(node_text(name, source));
    append_field(&mut label, node, "type_parameters", source, false);
    label.push_str(node_text(parameters, source));
    append_field(&mut label, node, "interfaces", source, true);
    Some(new_symbol_entry(
        Section::Class,
        node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&label), SIGNATURE_LIMIT),
    ))
}

fn extract_annotation_type(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut label = declaration_prefix(node, source);
    label.push_str("@interface ");
    label.push_str(node_text(name, source));
    Some(new_symbol_entry(
        Section::Type,
        node,
        node_text(name, source).to_owned(),
        truncate(&compact_whitespace(&label), SIGNATURE_LIMIT),
    ))
}

fn extract_members(body: Node<'_>, source: &[u8], interface: bool) -> Vec<super::EntryChild> {
    let mut members = Vec::new();
    let mut field_count = 0;
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method_declaration" => {
                if let (Some(signature), Some(name)) = (
                    method_signature(member, source),
                    member.child_by_field_name("name"),
                ) {
                    members.push(ranged_symbol_child(
                        signature,
                        member,
                        node_text(name, source).to_owned(),
                    ));
                }
            }
            "constructor_declaration" if !interface => {
                if let (Some(signature), Some(name)) = (
                    constructor_signature(member, source),
                    member.child_by_field_name("name"),
                ) {
                    members.push(ranged_symbol_child(
                        signature,
                        member,
                        node_text(name, source).to_owned(),
                    ));
                }
            }
            "field_declaration" if !interface => {
                field_count += 1;
                if field_count <= super::FIELD_TRUNCATE_THRESHOLD
                    && let Some(signature) = field_signature(member, source)
                {
                    members.push(ranged_child(signature, member));
                }
            }
            "constant_declaration" if interface => {
                field_count += 1;
                if field_count <= super::FIELD_TRUNCATE_THRESHOLD
                    && let Some(signature) = field_signature(member, source)
                {
                    members.push(ranged_child(signature, member));
                }
            }
            _ => {}
        }
    }
    truncate_child_count(&mut members, field_count);
    members
}

fn method_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    let return_type = node.child_by_field_name("type")?;
    let name = node.child_by_field_name("name")?;
    let parameters = node.child_by_field_name("parameters")?;
    let mut parts = signature_prefix(node, source);
    if let Some(type_parameters) = node.child_by_field_name("type_parameters") {
        parts.push(node_text(type_parameters, source).to_owned());
    }
    parts.push(node_text(return_type, source).to_owned());

    let mut declarator = format!(
        "{}{}",
        node_text(name, source),
        node_text(parameters, source)
    );
    if let Some(dimensions) = node.child_by_field_name("dimensions") {
        declarator.push_str(node_text(dimensions, source));
    }
    parts.push(declarator);
    if let Some(throws) = find_child(node, "throws") {
        parts.push(node_text(throws, source).to_owned());
    }
    Some(truncate(
        &compact_whitespace(&parts.join(" ")),
        SIGNATURE_LIMIT,
    ))
}

fn constructor_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let parameters = node.child_by_field_name("parameters")?;
    let mut parts = signature_prefix(node, source);
    if let Some(type_parameters) = node.child_by_field_name("type_parameters") {
        parts.push(node_text(type_parameters, source).to_owned());
    }
    parts.push(format!(
        "{}{}",
        node_text(name, source),
        node_text(parameters, source)
    ));
    if let Some(throws) = find_child(node, "throws") {
        parts.push(node_text(throws, source).to_owned());
    }
    Some(truncate(
        &compact_whitespace(&parts.join(" ")),
        SIGNATURE_LIMIT,
    ))
}

fn field_signature(node: Node<'_>, source: &[u8]) -> Option<String> {
    let field_type = node.child_by_field_name("type")?;
    let declarator = node.child_by_field_name("declarator")?;
    let name = declarator.child_by_field_name("name")?;
    let mut parts = signature_prefix(node, source);
    parts.push(node_text(field_type, source).to_owned());

    let mut declarator_text = node_text(name, source).to_owned();
    if let Some(dimensions) = declarator.child_by_field_name("dimensions") {
        declarator_text.push_str(node_text(dimensions, source));
    }
    parts.push(declarator_text);
    Some(truncate(
        &compact_whitespace(&parts.join(" ")),
        SIGNATURE_LIMIT,
    ))
}

fn declaration_prefix(node: Node<'_>, source: &[u8]) -> String {
    let modifiers = modifiers_text(node, source);
    if modifiers.is_empty() {
        String::new()
    } else {
        format!("{modifiers} ")
    }
}

fn signature_prefix(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let modifiers = modifiers_text(node, source);
    if modifiers.is_empty() {
        Vec::new()
    } else {
        vec![modifiers]
    }
}

fn modifiers_text(node: Node<'_>, source: &[u8]) -> String {
    let Some(modifiers) = find_child(node, "modifiers") else {
        return String::new();
    };
    let mut parts = Vec::new();
    let mut cursor = modifiers.walk();
    for modifier in modifiers.children(&mut cursor) {
        let text = node_text(modifier, source);
        if matches!(modifier.kind(), "marker_annotation" | "annotation")
            || matches!(
                text,
                "public"
                    | "private"
                    | "protected"
                    | "static"
                    | "final"
                    | "abstract"
                    | "default"
                    | "synchronized"
            )
        {
            parts.push(text);
        }
    }
    parts.join(" ")
}

fn append_field(text: &mut String, node: Node<'_>, field: &str, source: &[u8], spaced: bool) {
    if let Some(value) = node.child_by_field_name(field) {
        if spaced {
            text.push(' ');
        }
        text.push_str(node_text(value, source));
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use super::super::{SourceLanguage, skeleton};

    fn index(source: &str) -> String {
        let language = SourceLanguage::Java;
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        skeleton(language, tree.root_node(), source.as_bytes())
    }

    #[test]
    fn extracts_java_declarations_and_elides_implementations() {
        let output = index(
            "package com.example;\n\
             import java.util.List;\n\
             import static java.util.Collections.emptyList;\n\
             public class Service<T> extends Base implements Runnable {\n\
               private final String name = makeName();\n\
               public Service(String name) throws IOException { this.name = name; }\n\
               @Override public <R> R convert(T value) throws IOException { return work(value); }\n\
             }\n\
             public interface Handler<T> extends Comparable<T> { int LIMIT = compute(); void run() throws Exception; }\n\
             public enum Direction implements Displayable { UP, DOWN }\n\
             public record Point<T>(T x, T y) implements Serializable {}\n\
             public @interface Marker {}",
        );

        assert!(output.contains("com.example"));
        assert!(output.contains("java.util.List"));
        assert!(output.contains("static java.util.Collections.emptyList"));
        assert!(output.contains("public class Service<T> extends Base implements Runnable"));
        assert!(output.contains("private final String name"));
        assert!(output.contains("public Service(String name) throws IOException"));
        assert!(output.contains("@Override public <R> R convert(T value) throws IOException"));
        assert!(output.contains("public interface Handler<T> extends Comparable<T>"));
        assert!(output.contains("int LIMIT"));
        assert!(output.contains("public enum Direction implements Displayable"));
        assert!(output.contains("UP, DOWN"));
        assert!(output.contains("public record Point<T>(T x, T y) implements Serializable"));
        assert!(output.contains("public @interface Marker"));
        assert!(!output.contains("makeName"));
        assert!(!output.contains("this.name"));
        assert!(!output.contains("work(value)"));
        assert!(!output.contains("compute()"));
    }

    #[test]
    fn truncates_class_fields_after_eight_declarations() {
        let output = index(
            "class Many {\n\
               int one; int two; int three; int four; int five;\n\
               int six; int seven; int eight; int nine;\n\
             }",
        );

        assert!(output.contains("int eight"));
        assert!(!output.contains("int nine"));
        assert!(output.contains("[1 more truncated]"));
    }
}
