# treesitter-index

`treesitter-index` is a command-line tool that uses
[Tree-sitter](https://tree-sitter.github.io/tree-sitter/) to produce a compact,
line-numbered overview of source code. Instead of printing every implementation
detail, it extracts the structure that is most useful for quickly understanding
a file or repository: imports, modules, constants, types, traits, functions,
classes, methods, and tests. This is most useful to conserve tokens with agentic
programming.

This tool was generated with the help of large language models (LLMs).

## Installation

```sh
cargo build --release
```

The binary will be available at `target/release/treesitter-index`.

[ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`) is also required when
using regular-expression filtering across multiple files.

## Usage

Put this instruction in your AGENTS.md

```
- When analyzing source files, prefer running `treesitter-index -g <glob> <files or directories...>` early to obtain a compact structural skeleton before reading the full file, and then perform targeted reads for implementation details. Line numbers are indicated in square brackets (e.g [5] means line 5, and [5-10] means lines 5 to 10). To filter for specific symbols us '-e', for example to search for classes or functions containing 'manager' or 'main' (-i for ignore case): `treesitter-index -g "*.ts" -i -e "manager|main" src`. Don't use '*' as the glob since that circumvents the default ignore rules.
```

### Examples

Index one file:

```sh
treesitter-index src/main.rs
```

Index a directory recursively, restricting the input to Rust files:

```sh
treesitter-index -g '*.rs' src
```

Show only top-level classes or functions whose names match a regular
expression:

```sh
treesitter-index -i -e 'manager|main' src
```

Inspect the complete Tree-sitter syntax tree as JSON or an S-expression:

```sh
treesitter-index --format json src/main.rs
treesitter-index --format sexp src/main.rs
```

### Options

| Option | Description |
| --- | --- |
| `-t, --type <TYPE>` | Set the source language. Required for standard input and overrides extension-based detection. |
| `--format <FORMAT>` | Select `skeleton`, `json`, or `sexp` output. Defaults to `skeleton`. |
| `-g, --glob <GLOB>` | Include or exclude files. Prefix an exclusion with `!`; rules are applied in order. |
| `-e, --regexp <REGEXP>` | Include matching top-level classes and functions. May be repeated and is available for skeleton output. |
| `-i, --ignore-case` | Make regular-expression matching case-insensitive. |
| `-h, --help` | Print command-line help. |
| `-V, --version` | Print the version. |

Directory traversal respects standard ignore files such as `.gitignore`.

## Acknowledgements

This project was adapted from [Maki](https://github.com/tontinton/maki).

## License

See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
