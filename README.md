# openMD

An open-source, Notion-like Markdown editor. Pure Rust, native UI via
[GPUI](https://github.com/zed-industries/zed) (the GPU-accelerated framework behind Zed).

> Status: early scaffold. `openmd-core` (block model + GFM round-trip) and
> `openmd-storage` (files + SQLite FTS index) work. The GPUI editor shell
> lands next (milestone 4).

## Layout

```
crates/
  core/      # Block/Page model, undoable ops, Markdown import/export (GFM)
  storage/   # Vault: one .md file per page + SQLite FTS index for search
  app/       # CLI today; GPUI native editor UI next
vault-example/  # sample vault to try things out
```

## Quickstart

```sh
cargo test                          # core + storage tests
cargo run -p openmd-app -- list vault-example
cargo run -p openmd-app -- search vault-example "task"
```

Pages are plain GFM files with a small frontmatter header:

```markdown
---
id: 01J0000000000000000000000
title: Welcome
---

# Welcome

- [ ] Try openMD
```

SQLite (`<vault>/.openmd/index.db`) is only an index — files are the source of truth.

## License

Dual-licensed under MIT or Apache-2.0, like Zed.
See `LICENSE-MIT` and `LICENSE-APACHE`.
