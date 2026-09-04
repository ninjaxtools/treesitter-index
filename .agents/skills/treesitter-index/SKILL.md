---
name: treesitter-index
description: Use treesitter-index when analyzing source files, exploring code structure, or locating top-level entities and class methods before targeted reads.
---

# Treesitter Index

When analyzing source files, prefer running `treesitter-index -g <glob> <files or directories...>` early to obtain a compact structural skeleton before reading the full file. Then perform targeted reads for implementation details.

Line numbers are indicated in square brackets, such as `[5]` for one line and `[5-10]` for a range.

To filter for specific top-level entities or class methods, use `-e`. Add `-i` for case-insensitive matching and `--match-imports` to include matching imports. For example:

```sh
treesitter-index -g "*.ts" -i -e "manager|main" src
```

Do not use `*` as the glob because it circumvents the default ignore rules.
