package io.github.hessesian.jarindexer.model

import kotlinx.serialization.SerialName
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
    /** Generic type parameter names, e.g. `["T", "R"]` for `fun <T, R> foo(…)`. Empty for non-generic symbols. */
    @SerialName("type_params")
    val typeParams: List<String> = emptyList(),
    /** Full extension receiver type including generics, e.g. `"ImmutableList<T>"`. Empty for non-extension symbols. */
    @SerialName("extension_receiver_type")
    val extensionReceiverType: String = "",
    /** True when the last value parameter is a function type (lambda). */
    @SerialName("trailing_lambda")
    val trailingLambda: Boolean = false,
    /** True when the declaration carries an `@Deprecated` annotation (kotlin or java). */
    val deprecated: Boolean = false,
)
