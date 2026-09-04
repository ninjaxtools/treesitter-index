This repository employs the use of LLMs for [automatic programming](https://antirez.com/news/159).

This readme is written by a human.

# treesitter-index

`treesitter-index` is a cli tool that uses [tree-sitter](https://tree-sitter.github.io/tree-sitter/) to generate a source code skeleton that strips most of the code implementation but retains many of the structurally useful information that can act as an index into the source code. The resulting index output is most useful to conserve tokens and improve the agentic programming experience.

## Installation

In a checked out repository:

```sh
cargo install --path .
```

Or to build a binary in `target/release/treesitter-index`.

```sh
cargo build --release
```

### ripgrep

[ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`) is used to pre-filter files when a regex pattern is used (`-e`) and more than one input file or folder is provided and needs to be available on the system.

## Usage

### AGENTS.md

Put this instruction in your AGENTS.md

```
- When analyzing source files, prefer running `treesitter-index -g <glob> <files or directories...>` early to obtain a compact structural skeleton before reading the full file, and then perform targeted reads for implementation details. Line numbers are indicated in square brackets (e.g [5] means line 5, and [5-10] means lines 5 to 10). To filter for specific symbols us '-e', for example to search for classes or functions containing 'manager' or 'main' (-i for ignore case): `treesitter-index -g "*.ts" -i -e "manager|main" src`. Don't use '*' as the glob since that circumvents the default ignore rules.
```

### Skill file

You can use the skill in `.agents/skills/treesitter-index/SKILL.md ` instead of the instruction as well.

### Examples

Index one file:

```sh
treesitter-index src/main.rs
```

Index a directory recursively, only rust files:

```sh
treesitter-index -g '*.rs' src
```

Filter for top-level symbols (i.e. classes, functions, markdown headings):

```sh
treesitter-index -i -e 'manager|main' src
```

Output the treesitter syntax tree instead of a skeleton:

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
| `-e, --regexp <REGEXP>` | Include matching classes, functions, and Markdown headings. May be repeated and is available for skeleton output. |
| `-i, --ignore-case` | Make regular-expression matching case-insensitive. |
| `-h, --help` | Print command-line help. |
| `-V, --version` | Print the version. |

Directory traversal respects standard ignore files such as `.gitignore`.

Supported languages are Python, JavaScript, JSX, TypeScript, TSX, Rust, Go,
Java, and Markdown (`.md` and `.markdown`).

## Acknowledgements

This project was adapted from [Maki](https://github.com/tontinton/maki).

## License

See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
