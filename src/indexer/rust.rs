use tree_sitter::Node;

use super::{
    ChildStyle, Entry, Section, compact_whitespace, find_child, new_import_entry, new_symbol_entry,
    new_symbols_entry, node_text, ranged_child, ranged_symbol_child, truncate,
    truncate_child_count,
};

const SIGNATURE_LIMIT: usize = 160;

pub(super) fn extract(node: Node<'_>, source: &[u8], attrs: &[String]) -> Vec<Entry> {
    let entry = match node.kind() {
        "use_declaration" => extract_use(node, source),
        "mod_item" => extract_module(node, source),
        "const_item" | "static_item" => extract_constant(node, source),
        "type_item" => extract_type_alias(node, source, attrs),
        "struct_item" | "enum_item" | "union_item" => extract_data_type(node, source, attrs),
        "trait_item" => extract_trait(node, source),
        "impl_item" => extract_impl(node, source),
        "foreign_mod_item" => return extract_foreign_mod(node, source),
        "function_item" | "function_signature_item" => extract_function(node, source),
        "macro_definition" => extract_macro(node, source),
        _ => None,
    };
    entry.into_iter().collect()
}

fn extract_use(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let argument = node.child_by_field_name("argument")?;
    let mut paths = Vec::new();
    expand_use(argument, source, &[], &mut paths);
    (!paths.is_empty()).then(|| new_import_entry(node, paths))
}

fn expand_use(node: Node<'_>, source: &[u8], prefix: &[String], paths: &mut Vec<Vec<String>>) {
    match node.kind() {
        "scoped_use_list" => {
            let mut next = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                next.extend(path_segments(node_text(path, source)));
            } else if node_text(node, source).trim_start().starts_with("::") {
                next.push(String::new());
            }
            if let Some(list) = node.child_by_field_name("list") {
                expand_use(list, source, &next, paths);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for item in node.named_children(&mut cursor) {
                expand_use(item, source, prefix, paths);
            }
        }
        "use_as_clause" => {
            let Some(path) = node.child_by_field_name("path") else {
                return;
            };
            let mut result = prefix.to_vec();
            result.extend(path_segments(node_text(path, source)));
            if let Some(alias) = node.child_by_field_name("alias")
                && let Some(last) = result.last_mut()
            {
                *last = format!("{last} as {}", node_text(alias, source));
            }
            if !result.is_empty() {
                paths.push(result);
            }
        }
        _ => {
            let mut result = prefix.to_vec();
            result.extend(path_segments(node_text(node, source)));
            if !result.is_empty() {
                paths.push(result);
            }
        }
    }
}

fn path_segments(value: &str) -> Vec<String> {
    let value = value.trim();
    let mut segments: Vec<_> = value
        .split("::")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    if value.starts_with("::") {
        segments.insert(0, String::new());
    }
    segments
}

fn extract_module(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    Some(new_symbol_entry(
        Section::Module,
        node,
        node_text(name, source).to_owned(),
        prefixed(visibility(node, source), node_text(name, source)),
    ))
}

fn extract_constant(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let type_node = node.child_by_field_name("type")?;
    let mut declaration = String::new();
    if node.kind() == "static_item" {
        declaration.push_str("static ");
        let qualifiers = String::from_utf8_lossy(&source[node.start_byte()..name.start_byte()]);
        if qualifiers.split_whitespace().any(|part| part == "ref") {
            declaration.push_str("ref ");
        }
        if qualifiers.split_whitespace().any(|part| part == "mut") {
            declaration.push_str("mut ");
        }
    }
    declaration.push_str(node_text(name, source));
    declaration.push_str(": ");
    declaration.push_str(node_text(type_node, source));
    Some(new_symbol_entry(
        Section::Constant,
        node,
        node_text(name, source).to_owned(),
        truncate(
            &compact_whitespace(&prefixed(visibility(node, source), &declaration)),
            SIGNATURE_LIMIT,
        ),
    ))
}

fn extract_type_alias(node: Node<'_>, source: &[u8], attrs: &[String]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut entry = new_symbol_entry(
        Section::Type,
        node,
        node_text(name, source).to_owned(),
        truncate(&declaration_before(node, None, source), SIGNATURE_LIMIT),
    );
    entry.attrs = relevant_attrs(attrs);
    Some(entry)
}

fn extract_data_type(node: Node<'_>, source: &[u8], attrs: &[String]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let kind = node.kind().trim_end_matches("_item");
    let mut text = format!("{kind} {}", node_text(name, source));
    append_field(&mut text, node, "type_parameters", source);
    append_direct_child(&mut text, node, "where_clause", source);

    let mut entry = new_symbol_entry(
        Section::Type,
        node,
        node_text(name, source).to_owned(),
        truncate(
            &compact_whitespace(&prefixed(visibility(node, source), &text)),
            SIGNATURE_LIMIT,
        ),
    );
    entry.attrs = relevant_attrs(attrs);

    if let Some(body) = node.child_by_field_name("body") {
        if body.kind() == "enum_variant_list" {
            entry.child_style = ChildStyle::Brief;
            extract_variants(body, source, &mut entry);
        } else {
            extract_fields(body, source, &mut entry);
        }
    }
    Some(entry)
}

fn extract_fields(body: Node<'_>, source: &[u8], entry: &mut Entry) {
    let mut fields = Vec::new();
    let mut cursor = body.walk();
    match body.kind() {
        "field_declaration_list" => {
            fields.extend(
                body.named_children(&mut cursor)
                    .filter(|child| child.kind() == "field_declaration"),
            );
        }
        "ordered_field_declaration_list" => {
            fields.extend(body.children_by_field_name("type", &mut cursor));
        }
        _ => return,
    }

    let total = fields.len();
    for (index, field) in fields.into_iter().enumerate() {
        let field_visibility = if field.kind() == "field_declaration" {
            visibility(field, source)
        } else {
            field
                .prev_named_sibling()
                .filter(|previous| previous.kind() == "visibility_modifier")
                .map(|previous| node_text(previous, source))
                .unwrap_or("")
        };
        if index >= super::FIELD_TRUNCATE_THRESHOLD && field_visibility.is_empty() {
            continue;
        }

        let text = if field.kind() == "field_declaration" {
            let Some(name) = field.child_by_field_name("name") else {
                continue;
            };
            let Some(type_node) = field.child_by_field_name("type") else {
                continue;
            };
            prefixed(
                field_visibility,
                &format!(
                    "{}: {}",
                    node_text(name, source),
                    node_text(type_node, source)
                ),
            )
        } else {
            prefixed(field_visibility, node_text(field, source))
        };
        entry.children.push(ranged_child(
            truncate(&compact_whitespace(&text), SIGNATURE_LIMIT),
            field,
        ));
    }
    if entry.children.len() < total {
        truncate_child_count(&mut entry.children, total);
    }
}

fn extract_variants(body: Node<'_>, source: &[u8], entry: &mut Entry) {
    let mut cursor = body.walk();
    let variants: Vec<_> = body
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "enum_variant")
        .collect();
    let total = variants.len();
    for variant in variants.into_iter().take(super::FIELD_TRUNCATE_THRESHOLD) {
        let Some(name) = variant.child_by_field_name("name") else {
            continue;
        };
        entry
            .children
            .push(ranged_child(node_text(name, source).to_owned(), variant));
    }
    truncate_child_count(&mut entry.children, total);
}

fn extract_trait(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    let mut text = node_text(name, source).to_owned();
    append_field(&mut text, node, "type_parameters", source);
    append_field(&mut text, node, "bounds", source);
    append_direct_child(&mut text, node, "where_clause", source);

    let mut entry = new_symbol_entry(
        Section::Trait,
        node,
        node_text(name, source).to_owned(),
        truncate(
            &compact_whitespace(&prefixed(visibility(node, source), &text)),
            SIGNATURE_LIMIT,
        ),
    );
    if let Some(body) = node.child_by_field_name("body") {
        extract_members(body, source, false, &mut entry);
    }
    Some(entry)
}

fn extract_impl(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let type_node = node.child_by_field_name("type")?;
    let mut text = String::new();
    append_field(&mut text, node, "type_parameters", source);
    if !text.is_empty() {
        text.push(' ');
    }
    let trait_node = node.child_by_field_name("trait");
    if let Some(trait_node) = trait_node {
        if impl_is_negative(node, trait_node, source) {
            text.push('!');
        }
        text.push_str(node_text(trait_node, source));
        text.push_str(" for ");
    }
    text.push_str(node_text(type_node, source));
    append_direct_child(&mut text, node, "where_clause", source);

    let mut symbol_names = vec![node_text(type_node, source).to_owned()];
    if let Some(trait_node) = trait_node {
        symbol_names.push(node_text(trait_node, source).to_owned());
    }
    let mut entry = new_symbols_entry(
        Section::Impl,
        node,
        symbol_names,
        truncate(&compact_whitespace(&text), SIGNATURE_LIMIT),
    );
    if let Some(body) = node.child_by_field_name("body") {
        extract_members(body, source, true, &mut entry);
    }
    Some(entry)
}

fn extract_members(body: Node<'_>, source: &[u8], include_visibility: bool, entry: &mut Entry) {
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let Some(name) = member.child_by_field_name("name") else {
            continue;
        };
        let text = match member.kind() {
            "function_item" | "function_signature_item" => {
                function_signature(member, source, include_visibility)
            }
            "associated_type" | "type_item" => Some(truncate(
                &declaration_before(member, None, source),
                SIGNATURE_LIMIT,
            )),
            "const_item" => member.child_by_field_name("type").map(|type_node| {
                truncate(
                    &compact_whitespace(&String::from_utf8_lossy(
                        &source[member.start_byte()..type_node.end_byte()],
                    )),
                    SIGNATURE_LIMIT,
                )
            }),
            _ => None,
        };
        if let Some(text) = text {
            entry.children.push(ranged_symbol_child(
                text,
                member,
                node_text(name, source).to_owned(),
            ));
        }
    }
}

fn extract_foreign_mod(node: Node<'_>, source: &[u8]) -> Vec<Entry> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new();
    };
    let context = declaration_before(node, Some(body), source);
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter_map(|member| {
            let mut entry = match member.kind() {
                "function_item" | "function_signature_item" => extract_function(member, source),
                "static_item" => extract_constant(member, source),
                "associated_type" => extract_type_alias(member, source, &[]),
                _ => None,
            }?;
            entry.text = truncate(&prefixed(&context, &entry.text), SIGNATURE_LIMIT);
            Some(entry)
        })
        .collect()
}

fn extract_function(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    Some(new_symbol_entry(
        Section::Function,
        node,
        node_text(name, source).to_owned(),
        function_signature(node, source, true)?,
    ))
}

fn function_signature(node: Node<'_>, source: &[u8], include_visibility: bool) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    let mut text = String::new();
    if include_visibility {
        let visibility = visibility(node, source);
        if !visibility.is_empty() {
            text.push_str(visibility);
            text.push(' ');
        }
    }
    if let Some(modifiers) = direct_child(node, "function_modifiers") {
        text.push_str(node_text(modifiers, source));
        text.push(' ');
    }
    text.push_str(node_text(name, source));
    append_field(&mut text, node, "type_parameters", source);
    if let Some(parameters) = node.child_by_field_name("parameters") {
        text.push_str(node_text(parameters, source));
    } else {
        text.push_str("()");
    }
    if let Some(return_type) = node.child_by_field_name("return_type") {
        text.push_str(" -> ");
        text.push_str(
            node_text(return_type, source)
                .trim_start_matches("->")
                .trim(),
        );
    }
    append_direct_child(&mut text, node, "where_clause", source);
    Some(truncate(&compact_whitespace(&text), SIGNATURE_LIMIT))
}

fn extract_macro(node: Node<'_>, source: &[u8]) -> Option<Entry> {
    let name = node.child_by_field_name("name")?;
    Some(new_symbol_entry(
        Section::Macro,
        node,
        node_text(name, source).to_owned(),
        format!("{}!", node_text(name, source)),
    ))
}

fn declaration_before(node: Node<'_>, end: Option<Node<'_>>, source: &[u8]) -> String {
    let end_byte = end.map_or(node.end_byte(), |end| end.start_byte());
    compact_whitespace(
        String::from_utf8_lossy(&source[node.start_byte()..end_byte])
            .trim()
            .trim_end_matches([';', ','])
            .trim(),
    )
}

fn visibility<'source>(node: Node<'_>, source: &'source [u8]) -> &'source str {
    direct_child(node, "visibility_modifier")
        .map(|visibility| node_text(visibility, source))
        .unwrap_or("")
}

fn prefixed(prefix: &str, text: &str) -> String {
    if prefix.is_empty() {
        text.to_owned()
    } else {
        format!("{prefix} {text}")
    }
}

fn append_field(text: &mut String, node: Node<'_>, field: &str, source: &[u8]) {
    if let Some(value) = node.child_by_field_name(field) {
        text.push_str(node_text(value, source));
    }
}

fn append_direct_child(text: &mut String, node: Node<'_>, kind: &str, source: &[u8]) {
    if let Some(value) = direct_child(node, kind) {
        text.push(' ');
        text.push_str(node_text(value, source));
    }
}

fn direct_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    find_child(node, kind)
}

fn impl_is_negative(node: Node<'_>, trait_node: Node<'_>, source: &[u8]) -> bool {
    String::from_utf8_lossy(&source[node.start_byte()..trait_node.start_byte()])
        .trim_end()
        .ends_with('!')
}

fn relevant_attrs(attrs: &[String]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.contains("derive") || attr.contains("cfg"))
        .cloned()
        .collect()
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
        let language = SourceLanguage::Rust;
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
    fn expands_nested_uses_and_elides_bodies() {
        let output = index(
            "use std::{fs, io::{self, Read as Reader}, net::*};\n\
             pub async fn load<T>(value: T) -> Result<T> where T: Send { todo!() }",
        );

        assert!(output.contains("std::{fs, io::{Read as Reader, self}, net::*}"));
        assert!(output.contains("pub async load<T>(value: T) -> Result<T> where T: Send"));
        assert!(!output.contains("todo!"));
    }

    #[test]
    fn extracts_type_headers_and_ranged_members() {
        let output = index(
            "#[derive(Debug)]\n\
             pub struct Item<T> where T: Copy { pub value: T }\n\
             trait Build<T>: Send where T: Copy { fn build(value: T) -> Self; }\n\
             impl<T> Build<T> for Item<T> where T: Copy { pub fn new(value: T) -> Self { Self { value } } }",
        );

        assert!(output.contains("#[derive(Debug)]"));
        assert!(output.contains("pub struct Item<T> where T: Copy"));
        assert!(output.contains("pub value: T [2]"));
        assert!(output.contains("Build<T>: Send where T: Copy"));
        assert!(output.contains("<T> Build<T> for Item<T> where T: Copy"));
        assert!(output.contains("pub new(value: T) -> Self [4]"));
        assert!(!output.contains("Self { value }"));
    }

    #[test]
    fn extracts_associated_declarations_without_constant_initializers() {
        let output = index(
            "trait Store {\n\
               type Item<'a>: Send where Self: 'a;\n\
               const LIMIT: usize;\n\
               const DEFAULT: usize = { 100 + 200 };\n\
             }\n\
             impl Store for Cache {\n\
               type Item<'a> = Vec<&'a str>;\n\
               const LIMIT: usize = { 300 + 400 };\n\
             }\n\
             impl Cache { pub const SIZE: usize = 99; }",
        );

        for expected in [
            "type Item<'a>: Send where Self: 'a [2]",
            "const LIMIT: usize [3]",
            "const DEFAULT: usize [4]",
            "type Item<'a> = Vec<&'a str> [7]",
            "const LIMIT: usize [8]",
            "pub const SIZE: usize [10]",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
        assert!(!output.contains("100 + 200"));
        assert!(!output.contains("300 + 400"));
        assert!(!output.contains("99"));
    }

    #[test]
    fn filters_trait_and_impl_members_by_bare_name() {
        let source = "trait Store {\n\
            type Item;\n\
            const LIMIT: usize;\n\
            fn load(&self) -> Self::Item;\n\
            fn skip(&self) {}\n\
        }\n\
        impl Store for Cache {\n\
            type Item = Vec<u8>;\n\
            const LIMIT: usize = 10;\n\
            fn load(&self) -> Self::Item {\n\
                todo!()\n\
            }\n\
            fn skip(&self) {}\n\
        }";

        for pattern in ["^load$", "^(Store|load)$"] {
            let output = index_matching(source, &[pattern]);
            assert!(output.contains("Store [1-6]"), "{output}");
            assert!(output.contains("Store for Cache [7-14]"), "{output}");
            assert!(output.contains("load(&self) -> Self::Item [4]"), "{output}");
            assert!(
                output.contains("load(&self) -> Self::Item [10-12]"),
                "{output}"
            );
            assert!(!output.contains("skip"));
            assert!(!output.contains("type Item"));
            assert!(!output.contains("const LIMIT"));
            assert!(!output.contains("todo!"));
        }
        let types = index_matching(source, &["^Item$"]);
        assert!(types.contains("type Item [2]"), "{types}");
        assert!(types.contains("type Item = Vec<u8> [8]"), "{types}");
        assert!(!types.contains("load("));
        assert!(!types.contains("const LIMIT"));

        let constants = index_matching(source, &["^LIMIT$"]);
        assert!(constants.contains("const LIMIT: usize [3]"), "{constants}");
        assert!(constants.contains("const LIMIT: usize [9]"), "{constants}");
        assert!(!constants.contains("type Item"));
        assert!(!constants.contains("load("));

        let parent = index_matching(source, &["^Store$"]);
        assert!(parent.contains("Store for Cache"), "{parent}");
        assert!(!parent.contains("load("));
        assert!(!parent.contains("skip("));
        assert!(!parent.contains("type Item"));
        assert!(!parent.contains("const LIMIT"));
    }

    #[test]
    fn extracts_foreign_symbols_with_abi_and_declaration_ranges() {
        let source = "unsafe extern \"C\" {\n\
            pub fn foreign_read(\n\
                buffer: *mut u8,\n\
            ) -> i32;\n\
            pub static mut STATE: i32;\n\
        }\n\
        extern \"system\" {\n\
            fn other();\n\
            static READY: bool;\n\
        }";
        let output = index(source);
        for expected in [
            "unsafe extern \"C\" pub foreign_read( buffer: *mut u8, ) -> i32 [2-4]",
            "unsafe extern \"C\" pub static mut STATE: i32 [5]",
            "extern \"system\" other() [8]",
            "extern \"system\" static READY: bool [9]",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in:\n{output}"
            );
        }
        let function = index_matching(source, &["^foreign_read$"]);
        assert!(
            function.contains("unsafe extern \"C\" pub foreign_read("),
            "{function}"
        );
        assert!(!function.contains("STATE"));
        assert!(!function.contains("other"));
        let state = index_matching(source, &["^STATE$"]);
        assert!(
            state.contains("unsafe extern \"C\" pub static mut STATE: i32 [5]"),
            "{state}"
        );
        assert!(!state.contains("foreign_read"));
        assert!(index_matching(source, &["^C$"]).is_empty());
    }

    #[test]
    fn extracts_opaque_foreign_types() {
        let source =
            "unsafe extern \"C\" {\n    type Opaque;\n    fn release(value: *mut Opaque);\n}";
        let output = index_matching(source, &["^Opaque$"]);
        assert_eq!(output, "types:\n  unsafe extern \"C\" type Opaque [2]\n");
        assert!(index(source).contains("type Opaque [2]"));
    }
}
