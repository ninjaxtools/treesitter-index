use tree_sitter::Node;

use super::{Entry, Section, compact_whitespace, new_symbol_entry, node_text};

pub(super) fn extract(node: Node<'_>, source: &[u8], _attrs: &[String]) -> Vec<Entry> {
    let mut headings = Vec::new();
    collect_headings(node, node, source, &mut headings);

    headings
        .iter()
        .enumerate()
        .map(|(index, &(heading, scope, level, ref title))| {
            let end_byte = headings[index + 1..]
                .iter()
                .find(|(_, next_scope, next_level, _)| *next_scope == scope && *next_level <= level)
                .map_or_else(
                    || {
                        let mut cursor = scope.walk();
                        while cursor.goto_last_child() {}
                        let last = cursor.node();
                        // A container can end after consuming only an ancestor's next-line prefix.
                        if last.kind() == "block_continuation"
                            && last.end_byte() == scope.end_byte()
                        {
                            last.start_byte()
                        } else {
                            scope.end_byte()
                        }
                    },
                    |(next, _, _, _)| {
                        // Exclude the next heading's container markers as well as its title.
                        next.start_byte() - next.start_position().column
                    },
                );
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
    scope: Node<'tree>,
    source: &[u8],
    headings: &mut Vec<(Node<'tree>, Node<'tree>, usize, String)>,
) {
    // Container headings neither close outer sections nor extend beyond their container.
    let scope = if matches!(node.kind(), "block_quote" | "list_item") {
        node
    } else {
        scope
    };
    if matches!(node.kind(), "atx_heading" | "setext_heading")
        && let Some((level, title)) = heading_parts(node, source)
    {
        headings.push((node, scope, level, title));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_headings(child, scope, source, headings);
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
    let mut content = node
        .child_by_field_name("heading_content")
        .map(|content| node_text(content, source))
        .unwrap_or_default();
    if node.kind() == "atx_heading" {
        content = content.trim_end_matches([' ', '\t', '\r', '\n']);
        let prefix = content.trim_end_matches('#');
        // Check the raw separator: escaped hashes and non-ASCII spaces are title content.
        if prefix.len() < content.len() && (prefix.is_empty() || prefix.ends_with([' ', '\t'])) {
            content = prefix;
        }
    }
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
    Some((level, compact_whitespace(content)))
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

    #[test]
    fn filters_atx_headings_without_optional_closing_hashes() {
        use regex::Regex;

        let language = SourceLanguage::Markdown;
        let source = "# Guide ### \t\nIntro.\n\n## Next ##\nMore.\n";
        let mut parser = Parser::new();
        parser.set_language(&language.grammar()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let output = super::super::skeleton_matching(
            language,
            tree.root_node(),
            source.as_bytes(),
            &[Regex::new("^Guide$").unwrap()],
        );

        assert_eq!(output, "headings:\n  # Guide [1-5]\n");
    }

    #[test]
    fn only_strips_valid_atx_closing_delimiters() {
        for (heading, title) in [
            ("# Guide ###", "# Guide"),
            ("# Guide\t###\t", "# Guide"),
            ("# ###", "#"),
            ("# Guide###", "# Guide###"),
            (r"# Guide \###", r"# Guide \###"),
            (r"# Guide #\#", r"# Guide #\#"),
            (r"# Guide \# ###", r"# Guide \#"),
            (r"# Guide \\###", r"# Guide \\###"),
            (r"# Guide \\ ###", r"# Guide \\"),
            ("# Guide ### text", "# Guide ### text"),
            ("# Guide\u{a0}###", "# Guide ###"),
            ("# Guide ###\u{a0}", "# Guide ###"),
            ("# Guide `###`", "# Guide `###`"),
        ] {
            assert_eq!(
                index(heading),
                format!("headings:\n  {title} [1]\n"),
                "{heading:?}"
            );
        }
        assert_eq!(
            index("Guide ###\n===\n"),
            "headings:\n  # Guide ### [1-2]\n"
        );
    }

    #[test]
    fn scopes_quoted_headings_without_terminating_outer_sections() {
        let output =
            index("# Guide\n\n> # Quote\n> quoted body\n\nOutside quote.\n\n## Next\nMore.\n");

        assert_eq!(
            output,
            "headings:\n  # Guide [1-9]\n  # Quote [3-4]\n  ## Next [8-9]\n"
        );
    }

    #[test]
    fn scopes_list_items_and_nested_quotes_independently() {
        let output = index(
            "# Guide\n\n- # First\n  Body.\n\n  > # Quote ###\n  > Quoted.\n\n  After quote.\n\n- ## Second\n  Second body.\n\nOutside list.\n\n## Next\nMore.\n",
        );

        assert_eq!(
            output,
            "headings:\n  # Guide [1-17]\n  # First [3-9]\n  # Quote [6-7]\n  ## Second [11-12]\n  ## Next [16-17]\n"
        );
    }

    #[test]
    fn scopes_sibling_headings_and_ignores_fences_inside_quotes() {
        let output = index(
            "# Guide\n\n> # One\n> Body.\n> ## Child\n> Child body.\n> # Two\n> ```markdown\n> # Hidden\n> ```\n\nOutside.\n",
        );

        assert_eq!(
            output,
            "headings:\n  # Guide [1-12]\n  # One [3-6]\n  ## Child [5-6]\n  # Two [7-10]\n"
        );
    }

    #[test]
    fn ends_nested_quote_before_outer_heading_continuation() {
        assert_eq!(
            index("> > # Inner\n> > Body\n> # Outer\n> Outer body\n"),
            "headings:\n  # Inner [1-2]\n  # Outer [3-4]\n"
        );
    }

    #[test]
    fn ends_quoted_list_items_before_sibling_continuations() {
        assert_eq!(
            index("> - # First\n>   Body\n> - # Second\n>   Second body\n"),
            "headings:\n  # First [1-2]\n  # Second [3-4]\n"
        );
    }

    #[test]
    fn preserves_final_content_without_a_newline() {
        for source in [
            "# Heading\nBody >",
            "> # Heading\n> Body >",
            "> > # Heading\n> > Body >",
            "> - # Heading\n>   Body >",
            "> > # Heading\n> >     >",
            "> - # Heading\n>       >",
        ] {
            assert_eq!(
                index(source),
                "headings:\n  # Heading [1-2]\n",
                "{source:?}"
            );
        }
    }
}
