package io.github.hessesian.jarindexer

import io.github.hessesian.jarindexer.model.SymbolEntry
import kotlinx.metadata.*
import kotlinx.metadata.jvm.*
import org.objectweb.asm.AnnotationVisitor
import org.objectweb.asm.ClassReader
import org.objectweb.asm.ClassVisitor
import org.objectweb.asm.Opcodes

// ── ASM: extract @kotlin.Metadata annotation bytes ───────────────────────────

private class StringArrayCollector(private val target: MutableList<String>) : AnnotationVisitor(Opcodes.ASM9) {
    override fun visit(name: String?, value: Any?) {
        if (value is String) target.add(value)
    }
}

private class IntArrayCollector(private val onDone: (IntArray) -> Unit) : AnnotationVisitor(Opcodes.ASM9) {
    private val values = mutableListOf<Int>()
    override fun visit(name: String?, value: Any?) {
        if (value is Int) values.add(value)
    }
    override fun visitEnd() = onDone(values.toIntArray())
}

private class MetadataAnnotationVisitor : AnnotationVisitor(Opcodes.ASM9) {
    var kind: Int = 1
    var metadataVersion: IntArray = intArrayOf()
    val data1 = mutableListOf<String>()
    val data2 = mutableListOf<String>()
    var extraString: String = ""
    var packageName: String = ""
    var extraInt: Int = 0

    override fun visit(name: String?, value: Any?) {
        when (name) {
            "k"  -> kind            = (value as? Int) ?: kind
            "mv" -> if (value is IntArray) metadataVersion = value.copyOf()  // pre-built array path
            "xi" -> extraInt        = (value as? Int) ?: extraInt
            "xs" -> extraString     = (value as? String) ?: extraString
            "pn" -> packageName     = (value as? String) ?: packageName
        }
    }

    override fun visitArray(name: String?): AnnotationVisitor = when (name) {
        "mv" -> IntArrayCollector { metadataVersion = it }
        "d1" -> StringArrayCollector(data1)
        "d2" -> StringArrayCollector(data2)
        else -> object : AnnotationVisitor(Opcodes.ASM9) {}  // no-op for bv and any future fields
    }

    fun toMetadata() = Metadata(
        kind            = kind,
        metadataVersion = metadataVersion,
        data1           = data1.toTypedArray(),
        data2           = data2.toTypedArray(),
        extraString     = extraString,
        packageName     = packageName,
        extraInt        = extraInt,
    )
}

private class ClassMetadataVisitor : ClassVisitor(Opcodes.ASM9) {
    var simpleClassName: String = ""
    var metadataVisitor: MetadataAnnotationVisitor? = null
    var isPublic: Boolean = false

    override fun visit(version: Int, access: Int, name: String, signature: String?, superName: String?, interfaces: Array<out String>?) {
        simpleClassName = name.substringAfterLast('/')
        isPublic = (access and Opcodes.ACC_PUBLIC) != 0
    }

    override fun visitAnnotation(descriptor: String, visible: Boolean): AnnotationVisitor? {
        if (descriptor == "Lkotlin/Metadata;") {
            return MetadataAnnotationVisitor().also { metadataVisitor = it }
        }
        return null
    }
}

// ── Type rendering ─────────────────────────────────────────────────────────────

private val FUNCTION_TYPE_REGEX = Regex("Function\\d+")

/// Returns true when the last value parameter of `fn` is a function type —
/// meaning the function supports trailing-lambda call syntax.
private fun KmFunction.hasTrailingLambda(): Boolean {
    val lastType = valueParameters.lastOrNull()?.type ?: return false
    val classifier = lastType.classifier as? KmClassifier.Class ?: return false
    return FUNCTION_TYPE_REGEX.matches(classifier.name.substringAfterLast('/'))
}

private fun KmType.render(typeParams: Map<Int, String> = emptyMap()): String {
    val c = classifier
    // Render FunctionN types as Kotlin lambda syntax: (A) -> R, suspend X.() -> R, etc.
    if (c is KmClassifier.Class && FUNCTION_TYPE_REGEX.matches(c.name.substringAfterLast('/'))) {
        return renderAsFunctionType(typeParams)
    }
    return buildString {
        when (c) {
            is KmClassifier.Class         -> append(c.name.substringAfterLast('/'))
            is KmClassifier.TypeAlias     -> append(c.name.substringAfterLast('/'))
            is KmClassifier.TypeParameter -> append(typeParams[c.id] ?: "T")
        }
        if (arguments.isNotEmpty()) {
            append('<')
            arguments.joinTo(this, ", ") { proj ->
                when {
                    proj.type == null -> "*"
                    proj.variance == KmVariance.IN  -> "in ${proj.type!!.render(typeParams)}"
                    proj.variance == KmVariance.OUT -> "out ${proj.type!!.render(typeParams)}"
                    else -> proj.type!!.render(typeParams)
                }
            }
            append('>')
        }
        if (isNullable) append('?')
    }
}

/**
 * Render a FunctionN type as idiomatic Kotlin lambda syntax.
 *
 * Examples:
 *  - `Function1<String, Unit>`           → `(String) -> Unit`
 *  - `Function1<CoroutineScope, Unit>`   → `CoroutineScope.() -> Unit`  (with @ExtensionFunctionType)
 *  - `Function2<CoroutineScope, Continuation<Unit>, Any?>` (isSuspend) → `suspend CoroutineScope.() -> Unit`
 */
private fun KmType.renderAsFunctionType(typeParams: Map<Int, String>): String {
    val hasReceiver = annotations.any { it.className == "kotlin/ExtensionFunctionType" }
    val args = arguments.mapNotNull { it.type }

    val body = buildString {
        if (isSuspend) {
            // Suspend: JVM-erased args are [receiver?, param1, ..., Continuation<R>, Any?]
            val continuationIdx = args.indexOfLast { t ->
                (t.classifier as? KmClassifier.Class)?.name == "kotlin/coroutines/Continuation"
            }
            if (continuationIdx >= 0) {
                val returnType = args[continuationIdx].arguments.firstOrNull()?.type
                val effectiveArgs = args.take(continuationIdx)
                append("suspend ")
                appendFunctionParams(effectiveArgs, hasReceiver, typeParams)
                append(" -> ")
                append(returnType?.render(typeParams) ?: "Unit")
            } else {
                appendRegularFunctionType(args, hasReceiver, typeParams)
            }
        } else {
            appendRegularFunctionType(args, hasReceiver, typeParams)
        }
    }
    return if (isNullable) "$body?" else body
}

private fun StringBuilder.appendRegularFunctionType(
    args: List<KmType>,
    hasReceiver: Boolean,
    typeParams: Map<Int, String>,
) {
    if (args.isEmpty()) { append("() -> Unit"); return }
    val params = args.dropLast(1)
    val returnType = args.last()
    appendFunctionParams(params, hasReceiver, typeParams)
    append(" -> ")
    append(returnType.render(typeParams))
}

private fun StringBuilder.appendFunctionParams(
    args: List<KmType>,
    hasReceiver: Boolean,
    typeParams: Map<Int, String>,
) {
    if (hasReceiver && args.isNotEmpty()) {
        append(args[0].render(typeParams))
        append(".(")
        args.drop(1).joinTo(this, ", ") { it.render(typeParams) }
        append(")")
    } else {
        append("(")
        args.joinTo(this, ", ") { it.render(typeParams) }
        append(")")
    }
}

private fun KmType.isUnit() =
    (classifier as? KmClassifier.Class)?.name == "kotlin/Unit"

// ── Signature builders ─────────────────────────────────────────────────────────

private fun buildTypeParamMap(
    classTypeParams: List<KmTypeParameter>,
    fnTypeParams: List<KmTypeParameter>,
): Map<Int, String> = (classTypeParams + fnTypeParams).associate { it.id to it.name }

private fun renderFunction(fn: KmFunction, receiver: KmType? = null, classTypeParams: List<KmTypeParameter> = emptyList()): String {
    val typeParams = buildTypeParamMap(classTypeParams, fn.typeParameters)
    return buildString {
        if (fn.isSuspend) append("suspend ")
        append("fun ")
        if (fn.typeParameters.isNotEmpty()) {
            append('<')
            fn.typeParameters.joinTo(this, ", ") { it.name }
            append("> ")
        }
        if (receiver != null) { append(receiver.render(typeParams)); append('.') }
        append(fn.name)
        append('(')
        fn.valueParameters.joinTo(this, ", ") { p -> "${p.name}: ${p.type?.render(typeParams) ?: "Any?"}" }
        append(')')
        val ret = fn.returnType
        if (!ret.isUnit()) { append(": "); append(ret.render(typeParams)) }
    }
}

private fun renderProperty(prop: KmProperty, receiver: KmType? = null, classTypeParams: List<KmTypeParameter> = emptyList()): String {
    val typeParams = buildTypeParamMap(classTypeParams, emptyList())
    return buildString {
        append(if (prop.isVar) "var " else "val ")
        if (receiver != null) { append(receiver.render(typeParams)); append('.') }
        append(prop.name)
        prop.returnType?.let { append(": "); append(it.render(typeParams)) }
    }
}

// ── Kotlin class/package → SymbolEntry list ───────────────────────────────────

/** Extract the list of type parameter names declared on a function, e.g. `["T", "R"]`. */
private fun functionTypeParamNames(fn: KmFunction): List<String> =
    fn.typeParameters.map { it.name }

/**
 * Render the extension receiver type of a function, substituting any class/fun type params.
 * Returns empty string for non-extension functions.
 * e.g. `fun <T> ImmutableList<T>.fastForEach(…)` → `"ImmutableList<T>"`
 */
private fun extensionReceiverRendered(fn: KmFunction, classTypeParams: List<KmTypeParameter>): String {
    val typeParamsMap = buildTypeParamMap(classTypeParams, fn.typeParameters)
    return fn.receiverParameterType?.render(typeParamsMap) ?: ""
}

/**
 * Render the extension receiver type of a property, e.g.
 * `val ViewModel.viewModelScope: CoroutineScope` → `"ViewModel"`.
 * Returns empty string for non-extension properties.
 */
private fun extensionReceiverRenderedForProp(prop: KmProperty, classTypeParams: List<KmTypeParameter>): String {
    val typeParamsMap = buildTypeParamMap(classTypeParams, emptyList())
    return prop.receiverParameterType?.render(typeParamsMap) ?: ""
}

private fun entriesFromClass(klass: KmClass): List<SymbolEntry> {
    val entries = mutableListOf<SymbolEntry>()
    val simpleName = klass.name.substringAfterLast('/')
    val containerName = simpleName

    val classKind = when {
        klass.kind == ClassKind.INTERFACE          -> "interface"
        klass.kind == ClassKind.OBJECT             -> "object"
        klass.kind == ClassKind.COMPANION_OBJECT   -> "object"
        klass.kind == ClassKind.ENUM_CLASS         -> "class"
        klass.kind == ClassKind.ANNOTATION_CLASS   -> "interface"
        else                                       -> "class"
    }
    val classDetail = if (klass.typeParameters.isEmpty()) {
        "$classKind $simpleName"
    } else {
        val tps = klass.typeParameters.joinToString(", ") { it.name }
        "$classKind $simpleName<$tps>"
    }
    entries += SymbolEntry(simpleName, classKind, "", classDetail)

    for (fn in klass.functions) {
        if (!fn.visibility.isPublicLike()) continue
        val recv = fn.receiverParameterType
        entries += SymbolEntry(
            fn.name, "fun", containerName, renderFunction(fn, recv, klass.typeParameters),
            typeParams = functionTypeParamNames(fn),
            extensionReceiverType = extensionReceiverRendered(fn, klass.typeParameters),
            trailingLambda = fn.hasTrailingLambda(),
        )
    }
    for (prop in klass.properties) {
        if (!prop.visibility.isPublicLike()) continue
        val recv = prop.receiverParameterType
        val kind = if (prop.isVar) "var" else "val"
        entries += SymbolEntry(prop.name, kind, containerName, renderProperty(prop, recv, klass.typeParameters),
            extensionReceiverType = extensionReceiverRenderedForProp(prop, klass.typeParameters),
        )
    }
    return entries
}

private fun entriesFromPackage(pkg: KmPackage, containerName: String): List<SymbolEntry> {
    val entries = mutableListOf<SymbolEntry>()
    for (fn in pkg.functions) {
        if (!fn.visibility.isPublicLike()) continue
        val recv = fn.receiverParameterType
        entries += SymbolEntry(
            fn.name, "fun", containerName, renderFunction(fn, recv),
            typeParams = functionTypeParamNames(fn),
            extensionReceiverType = extensionReceiverRendered(fn, emptyList()),
            trailingLambda = fn.hasTrailingLambda(),
        )
    }
    for (prop in pkg.properties) {
        if (!prop.visibility.isPublicLike()) continue
        val recv = prop.receiverParameterType
        val kind = if (prop.isVar) "var" else "val"
        entries += SymbolEntry(prop.name, kind, containerName, renderProperty(prop, recv),
            extensionReceiverType = extensionReceiverRenderedForProp(prop, emptyList()),
        )
    }
    return entries
}

private fun Visibility?.isPublicLike() =
    this == Visibility.PUBLIC || this == Visibility.PROTECTED || this == null

// ── Java fallback (no Kotlin metadata) ────────────────────────────────────────

private class JavaClassVisitor(private val entries: MutableList<SymbolEntry>) : ClassVisitor(Opcodes.ASM9) {
    private var className = ""
    private var isPublicClass = false

    override fun visit(version: Int, access: Int, name: String, signature: String?, superName: String?, interfaces: Array<out String>?) {
        className = name.substringAfterLast('/')
        isPublicClass = (access and Opcodes.ACC_PUBLIC) != 0
        if (isPublicClass && !className.contains('$')) {
            entries += SymbolEntry(className, "class", "", "class $className")
        }
    }

    override fun visitMethod(access: Int, name: String, descriptor: String, signature: String?, exceptions: Array<out String>?): org.objectweb.asm.MethodVisitor? {
        if (!isPublicClass) return null
        val isPublic = (access and Opcodes.ACC_PUBLIC) != 0
        val isSynthetic = (access and Opcodes.ACC_SYNTHETIC) != 0
        val isBridge = (access and Opcodes.ACC_BRIDGE) != 0
        if (!isPublic || isSynthetic || isBridge || name == "<init>" || name == "<clinit>") return null
        if (name.contains('$')) return null
        entries += SymbolEntry(name, "fun", className, "fun $name(...)")
        return null
    }
}

// ── Public entry point ─────────────────────────────────────────────────────────

/**
 * Index a single `.class` file bytes → list of SymbolEntry.
 * Returns empty list on any error (corrupted class, synthetic inner, etc.).
 */
fun indexClassBytes(bytes: ByteArray): List<SymbolEntry> {
    return try {
        val visitor = ClassMetadataVisitor()
        ClassReader(bytes).accept(visitor, ClassReader.SKIP_CODE or ClassReader.SKIP_DEBUG or ClassReader.SKIP_FRAMES)

        val name = visitor.simpleClassName
        val metaVisitor = visitor.metadataVisitor
        if (metaVisitor != null) {
            // Kotlin class: visibility is controlled by metadata, not JVM access flags.
            // FileFacade/MultiFileClassPart helper classes are ACC_SYNTHETIC but their members are public.
            val metadata = runCatching { KotlinClassMetadata.readLenient(metaVisitor.toMetadata()) }.getOrNull()
                ?: return emptyList()
            val isFacade = metadata is KotlinClassMetadata.FileFacade || metadata is KotlinClassMetadata.MultiFileClassPart
            // Skip anonymous/inner synthetic helpers for regular Class only
            if (!isFacade && name.contains('$') && !name.endsWith("\$Companion")) return emptyList()
            when (metadata) {
                is KotlinClassMetadata.Class              -> entriesFromClass(metadata.kmClass)
                is KotlinClassMetadata.FileFacade         -> entriesFromPackage(metadata.kmPackage, name)
                is KotlinClassMetadata.MultiFileClassPart -> entriesFromPackage(metadata.kmPackage, name)
                else -> emptyList()
            }
        } else {
            // Pure Java: use JVM access flags
            if (!visitor.isPublic) return emptyList()
            val entries = mutableListOf<SymbolEntry>()
            ClassReader(bytes).accept(JavaClassVisitor(entries), ClassReader.SKIP_CODE or ClassReader.SKIP_DEBUG)
            entries
        }
    } catch (_: Exception) {
        emptyList()
    }
}
