package io.github.hessesian.jarindexer.model

import kotlinx.serialization.Serializable

/**
 * Mirrors Rust's SymbolEntry — fields the kotlin-lsp indexer understands.
 * `file` and `range` are omitted: the Rust side fills them in (empty/zero for JAR symbols).
 */
@Serializable
data class SymbolEntry(
    val name: String,
    /** "class", "interface", "object", "fun", "val", "var", "typealias" */
    val kind: String,
    /** Enclosing class/object name; empty for top-level symbols. */
    val container: String,
    /** Truncated declaration signature, e.g. "fun foo(x: Int): String" */
    val detail: String,
    /** KDoc documentation text, empty when sources JAR is not available. */
    val doc: String = "",
)
