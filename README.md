This repository employs the use of LLMs for [automatic programming](https://antirez.com/news/159).

This readme is written by a human.

# treesitter-index

`treesitter-index` is a cli tool that uses [tree-sitter](https://tree-sitter.github.io/tree-sitter/) to generate a source code skeleton that strips most of the code implementation but retains many of the structurally useful information that can act as an index into the source code. The resulting index output is most useful to conserve tokens and improve the agentic programming experience.

## Installation

You can download a binary from one of the releases: https://github.com/ninjaxtools/treesitter-index/releases

Or you can build from source:

```sh
cargo install --path .
```

### ripgrep

[ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`) is used to pre-filter files when a regex pattern is used (`-e`) and more than 10 files are to be processed. The `rg` binary must be available on the system. This should be turned off with `--no-prefilter` if precise matches are desired.

## Usage

### AGENTS.md

Put this instruction in your AGENTS.md

```
- When analyzing source files, prefer running `treesitter-index -g <glob> <files or directories...>` early to obtain a compact structural skeleton before reading the full file, and then perform targeted reads for implementation details. Line numbers are indicated in square brackets (e.g [5] means line 5, and [5-10] means lines 5 to 10). To filter for specific top-level symbols, use `-e`, for example: `treesitter-index -g "*.ts" -i -e "manager|main" src`. Add `--match-imports` to include matching imports. Don't use `*` as the glob since that circumvents the default ignore rules.
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

Filter for top-level symbols (i.e. classes, class methods, functions, markdown headings):

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
| `-e, --regexp <REGEXP>` | Include matching top-level symbols. May be repeated and is available for skeleton output. |
| `--match-imports` | Also match and include imports when using `--regexp`. |
| `--no-prefilter` | Disable prefiltering with ripgrep. |
| `-i, --ignore-case` | Make regular-expression matching case-insensitive. |
| `-h, --help` | Print command-line help. |
| `-V, --version` | Print the version. |

Directory traversal respects standard ignore files such as `.gitignore`.

Supported languages are Python, JavaScript, JSX, TypeScript, TSX, Rust, Go,
Java, and Markdown (`.md` and `.markdown`).

## Releasing

Release a new binary distribution on github with:

```sh
cargo release patch --no-publish --execute
```

## Related Work

- [CoderLM: REPL to API Mapping](https://github.com/JaredStewart/coderlm/blob/main/server/REPL_to_API.md)
- [Aider: Building a better repository map with tree sitter](https://aider.chat/2023/10/22/repomap.html)
- [Maki: Token Economy](https://maki.sh/docs/token-economy/)

## Acknowledgements

This project was adapted from [Maki](https://github.com/tontinton/maki).

## License

See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).
