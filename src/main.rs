mod indexer;

use std::{
    collections::HashSet,
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use ignore::{
    WalkBuilder,
    overrides::{Override, OverrideBuilder},
};
use indexer::SourceLanguage;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use tree_sitter::{Node, Parser, Point};

const HELP: &str = "\
Create a source-code skeleton using Tree-sitter.

Usage: treesitter-index [OPTIONS] [PATH]...

Arguments:
  [PATH]...  Source files or directories to index recursively, or - for stdin [default: stdin]

Options:
  -t, --type <TYPE>                Source language; required for stdin; overrides file extension
      --format <FORMAT>            Output format: skeleton, json, or sexp [default: skeleton]
  -g, --glob <GLOB>                Include or exclude files; prefix exclusions with !
  -e, --regexp <REGEXP>            Output matching top-level entities; repeatable
      --match-imports              Also match and output imports with --regexp
  -i, --ignore-case                Make regexp matching case insensitive
  -h, --help                       Print help
  -V, --version                    Print version

Glob rules are applied in order and the last matching rule wins.
Unmatched files are excluded when any inclusion glob is given.

Languages: python, javascript, jsx, typescript, tsx, rust, go, java, markdown
";

#[derive(Clone, Copy)]
enum OutputFormat {
    Skeleton,
    Json,
    Sexp,
}

struct Args {
    format: OutputFormat,
    language: Option<SourceLanguage>,
    files: Vec<PathBuf>,
    glob_values: Vec<String>,
    regexps: Vec<Regex>,
    ignore_case: bool,
    match_imports: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ParseResult<'tree> {
    language: &'static str,
    has_error: bool,
    tree: SyntaxNode<'tree>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyntaxNode<'tree> {
    kind: &'tree str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'tree str>,
    named: bool,
    extra: bool,
    missing: bool,
    error: bool,
    start_byte: usize,
    end_byte: usize,
    start_position: Position,
    end_position: Position,
    children: Vec<SyntaxNode<'tree>>,
}

#[derive(Serialize)]
struct Position {
    row: usize,
    column: usize,
}

impl From<Point> for Position {
    fn from(point: Point) -> Self {
        Self {
            row: point.row,
            column: point.column,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("treesitter-index: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let Some(args) = parse_args(env::args().skip(1))? else {
        return Ok(());
    };

    let files: Vec<Option<PathBuf>> = if args.files.is_empty() {
        vec![None]
    } else {
        expand_paths(&args.files, &args.glob_values)?
            .into_iter()
            .map(Some)
            .collect()
    };
    let multiple = files.len() > 1;
    let mut stderr = io::stderr().lock();
    let mut inputs = Vec::new();

    for file in &files {
        let file = file.as_deref();
        let Some(language) = resolve_language(&mut stderr, args.language, file)? else {
            continue;
        };
        inputs.push((file, language));
    }

    if multiple && !args.regexps.is_empty() {
        let paths: Vec<_> = inputs
            .iter()
            .filter_map(|(file, _)| file.filter(|path| *path != Path::new("-")))
            .collect();
        let matching_files = rg_matching_files(&paths, &args.regexps, args.ignore_case)?;
        inputs.retain(|(file, _)| {
            file.is_none_or(|path| path == Path::new("-") || matching_files.contains(path))
        });
    }

    let mut stdout = io::stdout().lock();
    let mut output_index = 0;
    for (file, language) in inputs {
        let source = read_source(file)?;
        if args.regexps.is_empty() {
            write_file_prefix(&mut stdout, file, output_index, multiple)?;
            write_index(&mut stdout, args.format, language, &source)?;
        } else {
            let tree = parse_source(&source, language)?;
            let skeleton = indexer::skeleton_matching_imports(
                language,
                tree.root_node(),
                &source,
                &args.regexps,
                args.match_imports,
            );
            if skeleton.is_empty() {
                continue;
            }
            write_file_prefix(&mut stdout, file, output_index, multiple)?;
            write!(stdout, "{skeleton}")
                .map_err(|error| format!("failed to write output: {error}"))?;
        }
        output_index += 1;
    }

    Ok(())
}

fn rg_matching_files(
    files: &[&Path],
    regexps: &[Regex],
    case_insensitive: bool,
) -> Result<HashSet<PathBuf>, String> {
    if files.is_empty() {
        return Ok(HashSet::new());
    }

    let mut command = Command::new("rg");
    command.args(["--no-config", "-l", "-0"]);
    if case_insensitive {
        command.arg("-i");
    }
    for regexp in regexps {
        command.arg("-e").arg(regexp_for_rg(regexp.as_str()));
    }
    command.arg("--").args(files);

    let output = command
        .output()
        .map_err(|error| format!("failed to run rg prefilter: {error}"))?;
    if !output.status.success() && output.status.code() != Some(1) {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("rg prefilter failed with {}", output.status)
        } else {
            format!("rg prefilter failed: {detail}")
        });
    }

    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_rg_output)
        .collect()
}

fn regexp_for_rg(regexp: &str) -> String {
    let mut result = String::new();
    let mut characters = regexp.chars().peekable();
    let mut class_depth: usize = 0;

    while let Some(character) = characters.next() {
        match character {
            '\\' if class_depth == 0 && matches!(characters.peek(), Some('A' | 'z')) => {
                characters.next();
            }
            '\\' => {
                result.push(character);
                if let Some(escaped) = characters.next() {
                    result.push(escaped);
                }
            }
            '[' => {
                class_depth += 1;
                result.push(character);
            }
            ']' => {
                class_depth = class_depth.saturating_sub(1);
                result.push(character);
            }
            '^' | '$' if class_depth == 0 => {}
            _ => result.push(character),
        }
    }

    result
}

#[cfg(unix)]
fn path_from_rg_output(path: &[u8]) -> Result<PathBuf, String> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    Ok(PathBuf::from(OsString::from_vec(path.to_vec())))
}

#[cfg(not(unix))]
fn path_from_rg_output(path: &[u8]) -> Result<PathBuf, String> {
    String::from_utf8(path.to_vec())
        .map(PathBuf::from)
        .map_err(|error| format!("rg returned a non-UTF-8 path: {error}"))
}

fn expand_paths(paths: &[PathBuf], glob_values: &[String]) -> Result<Vec<PathBuf>, String> {
    let current_dir = env::current_dir()
        .map_err(|error| format!("failed to resolve current directory: {error}"))?;
    let overrides = build_overrides(&current_dir, glob_values)?;
    let mut files = Vec::new();
    for path in paths {
        if path == Path::new("-") {
            files.push(path.clone());
            continue;
        }

        let metadata = fs::metadata(path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if metadata.is_dir() {
            collect_directory_files(path, &overrides, &mut files)?;
        } else if !overrides.matched(path, false).is_ignore() {
            files.push(path.clone());
        }
    }
    Ok(files)
}

fn build_overrides(root: &Path, glob_values: &[String]) -> Result<Override, String> {
    let mut override_builder = OverrideBuilder::new(root);
    for value in glob_values {
        override_builder
            .add(value)
            .map_err(|error| format!("invalid glob pattern {value:?}: {error}"))?;
    }
    override_builder
        .build()
        .map_err(|error| format!("failed to build glob matcher: {error}"))
}

fn collect_directory_files(
    directory: &Path,
    overrides: &Override,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let mut builder = WalkBuilder::new(directory);
    builder.overrides(overrides.clone());
    builder.sort_by_file_name(|left, right| left.cmp(right));

    for entry in builder.build().skip(1) {
        let entry = entry.map_err(|error| {
            format!(
                "failed to traverse directory {}: {error}",
                directory.display()
            )
        })?;
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            files.push(entry.into_path());
        }
    }
    Ok(())
}

fn write_file_prefix(
    output: &mut impl Write,
    file: Option<&Path>,
    index: usize,
    multiple: bool,
) -> Result<(), String> {
    if index > 0 {
        writeln!(output).map_err(|error| format!("failed to write output: {error}"))?;
    }
    if let Some(file) = file.filter(|_| multiple) {
        writeln!(output, "{}", file.display())
            .map_err(|error| format!("failed to write output: {error}"))?;
    }
    Ok(())
}

fn write_index(
    output: &mut impl Write,
    format: OutputFormat,
    language: SourceLanguage,
    source: &[u8],
) -> Result<(), String> {
    let tree = parse_source(source, language)?;
    let root = tree.root_node();

    match format {
        OutputFormat::Skeleton => {
            write!(output, "{}", indexer::skeleton(language, root, source))
                .map_err(|error| format!("failed to write output: {error}"))?;
        }
        OutputFormat::Json => {
            let result = ParseResult {
                language: language.name(),
                has_error: root.has_error(),
                tree: syntax_node(root, None),
            };
            serde_json::to_writer(&mut *output, &result)
                .map_err(|error| format!("failed to write JSON: {error}"))?;
            writeln!(output).map_err(|error| format!("failed to write output: {error}"))?;
        }
        OutputFormat::Sexp => {
            writeln!(output, "{}", root.to_sexp())
                .map_err(|error| format!("failed to write output: {error}"))?;
        }
    }

    Ok(())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut format = OutputFormat::Skeleton;
    let mut language = None;
    let mut files = Vec::new();
    let mut glob_values = Vec::new();
    let mut regexp_values = Vec::new();
    let mut ignore_case = false;
    let mut match_imports = false;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("treesitter-index {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--format" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--format requires skeleton, json, or sexp".to_owned())?;
                format = parse_format(&value)?;
            }
            "-t" | "--type" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--type requires a language name".to_owned())?;
                language = Some(SourceLanguage::from_name(&value)?);
            }
            "-g" | "--glob" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--glob requires a glob pattern".to_owned())?;
                glob_values.push(value);
            }
            "-e" | "--regexp" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--regexp requires a regular expression".to_owned())?;
                regexp_values.push(value);
            }
            "-i" | "--ignore-case" => ignore_case = true,
            "--match-imports" => match_imports = true,
            "-" => files.push(PathBuf::from("-")),
            _ if arg.starts_with("--format=") => {
                format = parse_format(&arg["--format=".len()..])?;
            }
            _ if arg.starts_with("--type=") => {
                language = Some(SourceLanguage::from_name(&arg["--type=".len()..])?);
            }
            _ if arg.starts_with("--glob=") => {
                glob_values.push(arg["--glob=".len()..].to_owned());
            }
            _ if arg.starts_with("--regexp=") => {
                regexp_values.push(arg["--regexp=".len()..].to_owned());
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }

    if !regexp_values.is_empty() && !matches!(format, OutputFormat::Skeleton) {
        return Err("--regexp is only supported with skeleton output".to_owned());
    }
    if match_imports && regexp_values.is_empty() {
        return Err("--match-imports requires --regexp".to_owned());
    }
    let regexps = regexp_values
        .into_iter()
        .map(|value| parse_regexp(&value, ignore_case))
        .collect::<Result<_, _>>()?;
    let current_dir = env::current_dir()
        .map_err(|error| format!("failed to resolve current directory: {error}"))?;
    build_overrides(&current_dir, &glob_values)?;

    Ok(Some(Args {
        format,
        language,
        files,
        glob_values,
        regexps,
        ignore_case,
        match_imports,
    }))
}

fn parse_regexp(value: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(value)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|error| format!("invalid regular expression {value:?}: {error}"))
}

fn parse_format(value: &str) -> Result<OutputFormat, String> {
    match value {
        "skeleton" => Ok(OutputFormat::Skeleton),
        "json" => Ok(OutputFormat::Json),
        "sexp" => Ok(OutputFormat::Sexp),
        _ => Err(format!("unsupported output format: {value}")),
    }
}

fn resolve_language(
    warnings: &mut impl Write,
    language: Option<SourceLanguage>,
    file: Option<&Path>,
) -> Result<Option<SourceLanguage>, String> {
    if let Some(language) = language {
        return Ok(Some(language));
    }
    match file {
        Some(path) if path != Path::new("-") => match SourceLanguage::from_path(path) {
            Ok(language) => Ok(Some(language)),
            Err(error) => {
                writeln!(
                    warnings,
                    "treesitter-index: warning: skipping {}: {error}",
                    path.display()
                )
                .map_err(|error| format!("failed to write warning: {error}"))?;
                Ok(None)
            }
        },
        _ => Err("--type is required when reading from stdin".to_owned()),
    }
}

fn read_source(file: Option<&Path>) -> Result<Vec<u8>, String> {
    match file {
        Some(path) if path != Path::new("-") => {
            fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
        }
        _ => {
            let mut source = Vec::new();
            io::stdin()
                .read_to_end(&mut source)
                .map_err(|error| format!("failed to read stdin: {error}"))?;
            Ok(source)
        }
    }
}

fn parse_source(source: &[u8], language: SourceLanguage) -> Result<tree_sitter::Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar())
        .map_err(|error| format!("failed to load {} grammar: {error}", language.name()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| "parser returned no syntax tree".to_owned())
}

fn syntax_node<'tree>(node: Node<'tree>, field: Option<&'tree str>) -> SyntaxNode<'tree> {
    let children = (0..node.child_count())
        .filter_map(|index| {
            node.child(index)
                .map(|child| syntax_node(child, node.field_name_for_child(index)))
        })
        .collect();

    SyntaxNode {
        kind: node.kind(),
        field,
        named: node.is_named(),
        extra: node.is_extra(),
        missing: node.is_missing(),
        error: node.is_error(),
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_position: node.start_position().into(),
        end_position: node.end_position().into(),
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_is_required_for_stdin() {
        let mut warnings = Vec::new();
        assert!(resolve_language(&mut warnings, None, None).is_err());
        assert_eq!(
            resolve_language(&mut warnings, Some(SourceLanguage::Python), None).unwrap(),
            Some(SourceLanguage::Python)
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn warns_and_skips_files_with_unknown_languages() {
        let mut warnings = Vec::new();

        assert_eq!(
            resolve_language(&mut warnings, None, Some(Path::new("Cargo.toml"))).unwrap(),
            None
        );
        assert_eq!(
            String::from_utf8(warnings).unwrap(),
            "treesitter-index: warning: skipping Cargo.toml: unsupported file extension: .toml\n"
        );
    }

    #[test]
    fn accepts_all_output_formats() {
        assert!(matches!(
            parse_format("skeleton"),
            Ok(OutputFormat::Skeleton)
        ));
        assert!(matches!(parse_format("json"), Ok(OutputFormat::Json)));
        assert!(matches!(parse_format("sexp"), Ok(OutputFormat::Sexp)));
        assert!(parse_format("xml").is_err());
    }

    #[test]
    fn accepts_multiple_input_files() {
        let args = parse_args(
            ["first.rs", "second.py", "third.ts"]
                .into_iter()
                .map(String::from),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            args.files,
            ["first.rs", "second.py", "third.ts"].map(PathBuf::from)
        );
    }

    #[test]
    fn accepts_short_option_variants() {
        let args = parse_args(
            [
                "--format", "json", "-t", "rust", "-g", "*.rs", "-g", "!main.rs", "lib.rs",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap()
        .unwrap();

        assert!(matches!(args.format, OutputFormat::Json));
        assert_eq!(args.language, Some(SourceLanguage::Rust));
        assert_eq!(args.files, [PathBuf::from("lib.rs")]);
        assert_eq!(args.glob_values, ["*.rs", "!main.rs"]);
    }

    #[test]
    fn accepts_repeated_symbol_regexps() {
        let args = parse_args(
            ["-e", "^load.*", "--regexp", "^Service$", "--regexp=Repo."]
                .into_iter()
                .map(String::from),
        )
        .unwrap()
        .unwrap();

        assert_eq!(args.regexps.len(), 3);
        assert!(args.regexps[0].is_match("loader"));
        assert!(args.regexps[1].is_match("Service"));
        assert!(args.regexps[2].is_match("Repo1"));
        assert!(!args.regexps[0].is_match("Loader"));
        assert!(!args.ignore_case);
        assert!(!args.match_imports);
        assert!(
            parse_args(
                ["--format=json", "--regexp=load"]
                    .into_iter()
                    .map(String::from)
            )
            .is_err()
        );
        assert!(parse_args(["--regexp"].into_iter().map(String::from)).is_err());
        assert!(parse_args(["--regexp=["].into_iter().map(String::from)).is_err());

        let insensitive = parse_args(["-i", "--regexp=^load$"].into_iter().map(String::from))
            .unwrap()
            .unwrap();
        assert!(insensitive.ignore_case);
        assert!(insensitive.regexps[0].is_match("Load"));

        let imports = parse_args(
            ["--match-imports", "--regexp=^Service$"]
                .into_iter()
                .map(String::from),
        )
        .unwrap()
        .unwrap();
        assert!(imports.match_imports);
        assert!(parse_args(["--match-imports"].into_iter().map(String::from)).is_err());

        assert!(parse_args(["--glob=["].into_iter().map(String::from)).is_err());
        for old_option in [
            "-f",
            "-l",
            "-x",
            "-c",
            "--language",
            "--include",
            "--exclude",
        ] {
            assert!(parse_args([old_option].into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn broadens_symbol_anchors_for_rg() {
        assert_eq!(regexp_for_rg("^Load$"), "Load");
        assert_eq!(regexp_for_rg("(?m:^Load$)"), "(?m:Load)");
        assert_eq!(regexp_for_rg(r"\ALoad\z"), "Load");
        assert_eq!(regexp_for_rg(r"^[A-Z$]+\$$"), r"[A-Z$]+\$");
    }

    #[test]
    fn rg_prefilters_files_with_any_matching_symbol_regexp() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let main = root.join("src/main.rs");
        let indexer = root.join("src/indexer.rs");
        let regexps = [
            parse_regexp("^parse_args$", false).unwrap(),
            parse_regexp("^render_skeleton$", false).unwrap(),
        ];

        assert_eq!(
            rg_matching_files(&[&main, &indexer], &regexps, false).unwrap(),
            HashSet::from([main, indexer])
        );

        let missing = [parse_regexp("symbol_that_is_not_present", false).unwrap()];
        assert!(
            rg_matching_files(&[&root.join("src/indexer.rs")], &missing, false)
                .unwrap()
                .is_empty()
        );

        let uppercase = [parse_regexp("^RENDER_SKELETON$", false).unwrap()];
        assert!(
            rg_matching_files(&[&root.join("src/indexer.rs")], &uppercase, false)
                .unwrap()
                .is_empty()
        );

        let insensitive = [parse_regexp("^RENDER_SKELETON$", true).unwrap()];
        assert_eq!(
            rg_matching_files(&[&root.join("src/indexer.rs")], &insensitive, true).unwrap(),
            HashSet::from([root.join("src/indexer.rs")])
        );
    }

    #[test]
    fn applies_glob_rules_in_order() {
        let args = parse_args(
            [
                "--glob",
                "*.rs",
                "--glob=!main.rs",
                "--glob=src/main.rs",
                "src/main.rs",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap()
        .unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let overrides = build_overrides(&root, &args.glob_values).unwrap();

        assert!(
            overrides
                .matched(root.join("src/other.py"), false)
                .is_ignore()
        );
        assert!(
            overrides
                .matched(root.join("src/lib.rs"), false)
                .is_whitelist()
        );
        assert!(
            overrides
                .matched(root.join("src/main.rs"), false)
                .is_whitelist()
        );
    }

    #[test]
    fn exclusion_only_globs_leave_unmatched_files_included() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let overrides = build_overrides(&root, &["!*.tmp".to_owned()]).unwrap();

        assert!(overrides.matched(root.join("src/main.rs"), false).is_none());
        assert!(
            overrides
                .matched(root.join("src/cache.tmp"), false)
                .is_ignore()
        );
    }

    #[test]
    fn path_globs_are_relative_to_the_current_directory() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let overrides = build_overrides(&root, &["src/main.rs".to_owned()]).unwrap();

        assert!(
            overrides
                .matched(root.join("src/main.rs"), false)
                .is_whitelist()
        );
        assert!(
            overrides
                .matched(root.join("project/src/main.rs"), false)
                .is_ignore()
        );
    }

    #[test]
    fn expands_mixed_files_and_directories_recursively_in_order() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let main = root.join("src/main.rs");
        let source = root.join("src");

        assert_eq!(
            expand_paths(&[main.clone(), source.clone()], &[]).unwrap(),
            [
                main.clone(),
                source.join("indexer/go.rs"),
                source.join("indexer/java.rs"),
                source.join("indexer/markdown.rs"),
                source.join("indexer/python.rs"),
                source.join("indexer/rust.rs"),
                source.join("indexer/typescript.rs"),
                source.join("indexer.rs"),
                main,
            ]
        );
    }

    #[test]
    fn recursive_expansion_uses_ripgrep_default_ignores() {
        let root = env::temp_dir().join(format!(
            "treesitter-index-ignore-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".git/info")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitignore"), "ignored.rs\n").unwrap();
        fs::write(root.join(".ignore"), "dot-ignored.rs\n").unwrap();
        fs::write(root.join(".git/info/exclude"), "excluded.rs\n").unwrap();
        for path in [
            ".hidden.rs",
            ".hidden/nested.rs",
            "dot-ignored.rs",
            "excluded.rs",
            "ignored.rs",
            "src/nested.rs",
            "visible.rs",
        ] {
            fs::write(root.join(path), "fn example() {}\n").unwrap();
        }
        assert_eq!(
            expand_paths(std::slice::from_ref(&root), &[]).unwrap(),
            [root.join("src/nested.rs"), root.join("visible.rs")]
        );
        assert_eq!(
            expand_paths(&[root.join("ignored.rs")], &[]).unwrap(),
            [root.join("ignored.rs")]
        );
        assert_eq!(
            expand_paths(std::slice::from_ref(&root), &["ignored.rs".to_owned()]).unwrap(),
            [root.join("ignored.rs")]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filters_recursively_expanded_files_with_globs() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = root.join("src");
        let globs = ["*.rs", "!*.rs", "python.rs"].map(String::from);

        assert_eq!(
            expand_paths(std::slice::from_ref(&source), &globs).unwrap(),
            [source.join("indexer/python.rs")]
        );
    }

    #[test]
    fn prefixes_and_separates_multiple_files() {
        let mut output = Vec::new();

        write_file_prefix(&mut output, Some(Path::new("first.rs")), 0, true).unwrap();
        writeln!(output, "first contents").unwrap();
        write_file_prefix(&mut output, Some(Path::new("second.py")), 1, true).unwrap();
        writeln!(output, "second contents").unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "first.rs\nfirst contents\n\nsecond.py\nsecond contents\n"
        );

        let mut single_output = Vec::new();
        write_file_prefix(&mut single_output, Some(Path::new("only.rs")), 0, false).unwrap();
        assert!(single_output.is_empty());
    }
}
