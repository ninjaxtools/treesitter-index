use tree_sitter::Node;

use super::{Entry, Section, compact_whitespace, new_symbol_entry, node_text};

pub(super) fn extract(node: Node<'_>, source: &[u8], _attrs: &[String]) -> Vec<Entry> {
    let mut headings = Vec::new();
    collect_headings(node, source, &mut headings);

    headings
        .iter()
        .enumerate()
        .map(|(index, &(heading, level, ref title))| {
            let end_byte = headings[index + 1..]
                .iter()
                .find(|(_, next_level, _)| *next_level <= level)
                .map_or(node.end_byte(), |(next, _, _)| next.start_byte());
            let text = if title.is_empty() {
                "#".repeat(level)
            } else {
                format!("{} {title}", "#".repeat(level))
            };
            let mut entry = new_symbol_entry(Section::Heading, heading, title.clone(), text);
            entry.line_end = content_end_line(heading, end_byte, source);
            entry
        })
        .collect()
}

fn collect_headings<'tree>(
    node: Node<'tree>,
    source: &[u8],
    headings: &mut Vec<(Node<'tree>, usize, String)>,
) {
    if matches!(node.kind(), "atx_heading" | "setext_heading")
        && let Some((level, title)) = heading_parts(node, source)
    {
        headings.push((node, level, title));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_headings(child, source, headings);
    }
}

fn content_end_line(heading: Node<'_>, end_byte: usize, source: &[u8]) -> usize {
    let content = &source[heading.start_byte()..end_byte];
    let last = content
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(0);
    heading.start_position().row
        + 1
        + content[..last]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
}

fn heading_parts(node: Node<'_>, source: &[u8]) -> Option<(usize, String)> {
    let content = node
        .child_by_field_name("heading_content")
        .map(|content| compact_whitespace(node_text(content, source)))
        .unwrap_or_default();
    let mut cursor = node.walk();
    let level = node
        .children(&mut cursor)
        .find_map(|child| match child.kind() {
            "atx_h1_marker" | "setext_h1_underline" => Some(1),
            "atx_h2_marker" | "setext_h2_underline" => Some(2),
            "atx_h3_marker" => Some(3),
            "atx_h4_marker" => Some(4),
            "atx_h5_marker" => Some(5),
            "atx_h6_marker" => Some(6),
            _ => None,
        })?;
    Some((level, content))
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use super::super::{SourceLanguage, skeleton};

    fn index(source: &str) -> String {
        let language = SourceLanguage::Markdown;
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        skeleton(language, tree.root_node(), source.as_bytes())
    }

    #[test]
    fn extracts_atx_and_setext_heading_sections() {
        let output = index(
            "# Guide\n\
             Intro text.\n\n\
             Installation\n\
             ------------\n\n\
             ### Linux *notes*\n\
             Details.\n\n\
             ## Usage\n\
             Run it.\n",
        );

        assert_eq!(
            output,
            "headings:\n  # Guide [1-11]\n  ## Installation [4-8]\n  ### Linux *notes* [7-8]\n  ## Usage [10-11]\n"
        );
    }

    #[test]
    fn ignores_heading_like_text_in_fenced_code_blocks() {
        let output = index("# Real\n\n```markdown\n# Not a heading\n```\n");

        assert_eq!(output, "headings:\n  # Real [1-5]\n");
    }

    #[test]
    fn filters_headings_by_title() {
        use regex::Regex;

        let language = SourceLanguage::Markdown;
        let source = "# Guide\n\n## Install\nSteps.\n\n## Usage\nRun it.\n";
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let output = super::super::skeleton_matching(
            language,
            tree.root_node(),
            source.as_bytes(),
            &[Regex::new("^Install$").unwrap()],
        );

        assert_eq!(output, "headings:\n  ## Install [3-4]\n");
    }
}
