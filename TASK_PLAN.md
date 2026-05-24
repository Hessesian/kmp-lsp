# kotlin-lsp 开发任务计划

## 🟢 Known UX Gaps (from `docs/features.md`)

- [x] 1. **Hover — 四重反引号 fence** _Trivial_  
  `src/backend/format.rs` — ` ``` ` → ` ```` `, 防止 Kotlin 反引号标识符破坏 Markdown  
  → [#10](https://github.com/qdsfdhvh/kotlin-lsp/pull/10)

- [x] 2. **Completion — METHOD 图标映射** _Trivial_  
  `src/resolver/complete.rs` — `symbol_kind_to_completion` 中 METHOD → `CompletionItemKind::METHOD` (当前错映射为 FUNCTION)  
  → [#11](https://github.com/qdsfdhvh/kotlin-lsp/pull/11)

- [x] 3. **FoldingRange — import 块折叠** _Low_  
  `src/backend/handlers.rs` — 检测连续 `import` 行，输出 `FoldingRangeKind::Imports`

- [x] 4. **FoldingRange — 块注释折叠** _Low_  
  `src/backend/handlers.rs` — 检测 `/* … */` 多行注释，输出 `FoldingRangeKind::Comment`

- [x] 5. **FoldingRange — collapsedText** _Trivial_  
  `src/backend/handlers.rs` — 每个 fold range 设置 `collapsed_text`（`"{...}"` / `"imports"` / `"// ..."` / `"/* ..."`）

  → [#12](https://github.com/qdsfdhvh/kotlin-lsp/pull/12)

## 🟡 Not Yet Implemented LSP Capabilities (from `docs/features.md`)

- [x] 6. **`textDocument/gotoDeclaration`** _Trivial_  
  已实现 — 等同于 `goto_definition`，直接在 `backend/mod.rs` 委托

- [x] 7. **`textDocument/onTypeFormatting`** _Low_  
  `src/backend/mod.rs` — 注册 capability + handler: 输入 `}` 时自动对齐缩进  
  → [#13](https://github.com/qdsfdhvh/kotlin-lsp/pull/13)

- [x] 8. **`textDocument/formatting`** _Low_  
  `src/backend/handlers.rs` — 委托 ktfmt / google-java-format / swift-format 子进程格式化全文  
  → [#14](https://github.com/qdsfdhvh/kotlin-lsp/pull/14)

- [x] 9. **`textDocument/selectionRange`** _Medium_  
  `src/backend/handlers.rs` — CST 节点边界智能选区扩展，带 5 个测试  
  → [#15](https://github.com/qdsfdhvh/kotlin-lsp/pull/15)

- [x] 10. **Call Hierarchy（incoming/outgoing calls）** _Medium_  
  `src/backend/handlers.rs` — prepareCallHierarchy + incomingCalls (rg-based) + outgoingCalls (CST-based)，带 7 个测试  
  → [#16](https://github.com/qdsfdhvh/kotlin-lsp/pull/16)

- [x] 11. **Completion deprecated tag** _Medium_  
  索引时检测 `@Deprecated` / `@deprecated`，标记 `CompletionItemTag::DEPRECATED`

- [x] 12. **Completion label_details** _Medium_  
  行内参数列表 + 右对齐返回类型（RA 风格）

- [x] 13. **CodeAction quick-fixes** _Medium_  
  补充: add missing import, generate override stubs, suppress warning

## 🔴 P0 — AI Agent Token Efficiency (from `docs/ai-agent-token-efficiency.md`)

- [x] 14. **`context` CLI 命令** _Medium_  
  一站式符号上下文：definition + hover + refs 组合输出

- [x] 15. **Call Hierarchy CLI 命令** _Medium_  
  `call-hierarchy`（rg-based callers 搜索）

- [x] 16. **Richer hover** _Medium_  
  补充：visibility、KDoc 第一段、data class 属性列表、deprecated 警告

## 🔵 P1 — Token Efficiency

- [x] 17. **`check` CLI 命令** _Low_  
  语法错误诊断，无需 LSP 会话即可输出 parse errors

- [x] 18. **Type hierarchy CLI 命令** _Low_  
  `type-hierarchy --subtypes` / `--supertypes`，复用已有的 subtype lookup

## ⚪ P2

- [x] 19. **`organize-imports` CLI 命令** _Medium_  
  排序、去重、删除未使用 import

---

> 进度: **19 / 19** · 最后更新: 2026-05-23
