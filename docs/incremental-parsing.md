# Incremental Parsing Investigation

## Current State

`kotlin-lsp` re-parses the entire file on every `didChange` event using tree-sitter.
For small files (<1000 lines) this is negligible (~3ms). For large files (>5000 lines)
parse time can reach 15-20ms.

## tree-sitter Incremental Parsing Support

tree-sitter 0.22+ supports `parser.parse_with(&mut old_tree, ...)` which reuses
the previous parse tree. Only changed regions are re-parsed. This is already
supported by our tree-sitter version.

## Implementation Sketch

```rust
// In live_tree.rs or doc_tracking
let new_tree = parser.parse_with(&mut old_tree, new_bytes, None)?;
// Replace old tree atomically
```

## Recommendation

Not critical for 2026. Current parse times are well within LSP response budgets
(<20ms for files up to 5000 lines). Enable incremental parsing when profiling
shows it's a bottleneck for very large files (>10K lines).
