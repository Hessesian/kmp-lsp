package io.github.hessesian.jarindexer

import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.DisplayName
import org.junit.jupiter.api.io.TempDir
import java.io.File
import java.util.jar.JarOutputStream
import java.util.zip.ZipEntry

class IndexerTest {

    /** Helper: create a minimal JAR containing the given .class entries (name → bytes). */
    private fun createTestJar(dir: File, name: String, entries: Map<String, ByteArray>): File {
        val jarFile = File(dir, name)
        JarOutputStream(jarFile.outputStream()).use { jos ->
            for ((entryName, bytes) in entries) {
                jos.putNextEntry(ZipEntry(entryName))
                jos.write(bytes)
                jos.closeEntry()
            }
        }
        return jarFile
    }

    /** Helper: create a minimal valid .class file bytes for a public class. */
    fun minimalClassBytes(className: String): ByteArray {
        // Minimal class file: magic (0xCAFEBABE), version 52 (Java 8), 1 public class with no methods
        // We use ASM to generate proper bytecode
        val cw = org.objectweb.asm.ClassWriter(0)
        cw.visit(
            org.objectweb.asm.Opcodes.V1_8,
            org.objectweb.asm.Opcodes.ACC_PUBLIC,
            className,
            null,
            "java/lang/Object",
            null
        )
        // Add default constructor
        val mv = cw.visitMethod(
            org.objectweb.asm.Opcodes.ACC_PUBLIC,
            "<init>",
            "()V",
            null,
            null
        )
        mv.visitCode()
        mv.visitVarInsn(org.objectweb.asm.Opcodes.ALOAD, 0)
        mv.visitMethodInsn(
            org.objectweb.asm.Opcodes.INVOKESPECIAL,
            "java/lang/Object",
            "<init>",
            "()V",
            false
        )
        mv.visitInsn(org.objectweb.asm.Opcodes.RETURN)
        mv.visitMaxs(1, 1)
        mv.visitEnd()
        cw.visitEnd()
        return cw.toByteArray()
    }

    @Test
    @DisplayName("indexJarFile returns empty list for nonexistent JAR")
    fun testNonexistentJar(@TempDir tmpDir: File) {
        val result = indexJarFile("/nonexistent/path/foo.jar")
        assertTrue(result.isEmpty(), "should return empty for missing file")
    }

    @Test
    @DisplayName("indexJarFile indexes a Java class from JAR")
    fun testJavaClass(@TempDir tmpDir: File) {
        val classBytes = minimalClassBytes("com/example/TestClass")
        val jarFile = createTestJar(tmpDir, "test.jar", mapOf("com/example/TestClass.class" to classBytes))

        val result = indexJarFile(jarFile.absolutePath)

        assertTrue(result.isNotEmpty(), "should index at least one symbol")
        assertTrue(result.any { it.name == "TestClass" && it.kind == "class" },
            "should find TestClass class entry; got: ${result.map { it.name }}")
    }

    @Test
    @DisplayName("indexClassBytes extracts a public method via the Java fallback path (no Kotlin metadata, no sources JAR)")
    fun testJavaPublicMethodExtraction() {
        // Mirrors android.jar's own shape: plain Java bytecode, no @kotlin.Metadata,
        // no sibling -sources.jar — exercises JavaClassVisitor.visitMethod, which no
        // existing fixture reached (minimalClassBytes only emits a skipped <init>).
        val cw = org.objectweb.asm.ClassWriter(0)
        cw.visit(
            org.objectweb.asm.Opcodes.V1_8,
            org.objectweb.asm.Opcodes.ACC_PUBLIC,
            "com/example/TestActivity",
            null,
            "java/lang/Object",
            null
        )
        val ctor = cw.visitMethod(org.objectweb.asm.Opcodes.ACC_PUBLIC, "<init>", "()V", null, null)
        ctor.visitCode()
        ctor.visitVarInsn(org.objectweb.asm.Opcodes.ALOAD, 0)
        ctor.visitMethodInsn(org.objectweb.asm.Opcodes.INVOKESPECIAL, "java/lang/Object", "<init>", "()V", false)
        ctor.visitInsn(org.objectweb.asm.Opcodes.RETURN)
        ctor.visitMaxs(1, 1)
        ctor.visitEnd()
        val finish = cw.visitMethod(org.objectweb.asm.Opcodes.ACC_PUBLIC, "finish", "()V", null, null)
        finish.visitCode()
        finish.visitInsn(org.objectweb.asm.Opcodes.RETURN)
        finish.visitMaxs(0, 0)
        finish.visitEnd()
        cw.visitEnd()

        val result = indexClassBytes(cw.toByteArray())

        assertTrue(result.any { it.name == "TestActivity" && it.kind == "class" },
            "should find the class entry; got: ${result.map { it.name }}")
        assertTrue(
            result.any { it.name == "finish" && it.kind == "fun" && it.container == "TestActivity" },
            "should find the public method via the Java fallback path; got: ${result.map { "${it.name}:${it.kind}:${it.container}" }}"
        )
    }

    @Test
    @DisplayName("indexClassBytes extracts a public static final field via the Java fallback path (no Kotlin metadata)")
    fun testJavaPublicFieldExtraction() {
        // Mirrors android.jar's own motivating example: Activity.RESULT_OK is a
        // plain Java `public static final int` field with no Kotlin metadata.
        val cw = org.objectweb.asm.ClassWriter(0)
        cw.visit(
            org.objectweb.asm.Opcodes.V1_8,
            org.objectweb.asm.Opcodes.ACC_PUBLIC,
            "com/example/TestActivity",
            null,
            "java/lang/Object",
            null
        )
        cw.visitField(
            org.objectweb.asm.Opcodes.ACC_PUBLIC or org.objectweb.asm.Opcodes.ACC_STATIC or org.objectweb.asm.Opcodes.ACC_FINAL,
            "RESULT_OK",
            "I",
            null,
            -1
        ).visitEnd()
        cw.visitEnd()

        val result = indexClassBytes(cw.toByteArray())

        assertTrue(
            result.any { it.name == "RESULT_OK" && it.kind == "val" && it.container == "TestActivity" },
            "should find the public static final field via the Java fallback path; got: ${result.map { "${it.name}:${it.kind}:${it.container}:${it.detail}" }}"
        )
    }

    @Test
    @DisplayName("indexClassBytes does not orphan fields on a synthetic (compiler-numbered) nested class")
    fun testJavaFieldExtractionSkipsSyntheticNestedClass() {
        // The class-visiting entry point excludes compiler-SYNTHESIZED `$`-named
        // classes (anonymous/local, numbered `Outer$1` — see
        // testJavaAnonymousNestedClassIsExcluded) but now indexes a REAL named
        // nested class (see testJavaNestedClassIsIndexed). Field extraction must
        // apply the same synthetic-only exclusion — otherwise fields on an
        // anonymous class get indexed under a container qualifier ("Outer$1")
        // that was never indexed as a class and that no Kotlin caller could ever
        // reference.
        val cw = org.objectweb.asm.ClassWriter(0)
        cw.visit(
            org.objectweb.asm.Opcodes.V1_8,
            org.objectweb.asm.Opcodes.ACC_PUBLIC,
            "com/example/Outer\$1",
            null,
            "java/lang/Object",
            null
        )
        cw.visitField(
            org.objectweb.asm.Opcodes.ACC_PUBLIC or org.objectweb.asm.Opcodes.ACC_STATIC or org.objectweb.asm.Opcodes.ACC_FINAL,
            "ORPHAN_FIELD",
            "I",
            null,
            -1
        ).visitEnd()
        cw.visitEnd()

        val result = indexClassBytes(cw.toByteArray())

        assertTrue(
            result.isEmpty(),
            "fields on an anonymous/synthetic \$-named class must be skipped, matching class-skipping behavior; got: ${result.map { "${it.name}:${it.kind}:${it.container}" }}"
        )
    }

    @Test
    @DisplayName("indexClassBytes captures package and top-level flag")
    fun testPackageAndTopLevel() {
        val result = indexClassBytes(minimalClassBytes("androidx/compose/runtime/Composables"))
        val cls = result.firstOrNull { it.name == "Composables" && it.kind == "class" }
        assertTrue(cls != null, "should index the class; got: ${result.map { it.name }}")
        assertEquals("androidx.compose.runtime", cls!!.pkg, "class package")
        assertTrue(cls.topLevel, "a class is a top-level declaration")
    }

    @Test
    @DisplayName("selectSourcesJar prefers real API sources over samples")
    fun testSelectSourcesJar() {
        // Compose ships both a samples jar and the real API sources for `ui`.
        val candidates = listOf(
            "ui-1.11.2-samples-sources.jar",
            "ui-android-1.11.2-sources.jar",
        )
        assertEquals(
            "ui-android-1.11.2-sources.jar",
            SourcesKdocReader.selectSourcesJar(candidates, "ui"),
            "must skip the samples jar and pick the real API sources",
        )
        // Only a samples jar → nothing usable.
        assertEquals(
            null,
            SourcesKdocReader.selectSourcesJar(listOf("ui-1.11.2-samples-sources.jar"), "ui"),
        )
        // Single real sources jar → that one.
        assertEquals(
            "kotlinx-coroutines-core-1.7.3-sources.jar",
            SourcesKdocReader.selectSourcesJar(listOf("kotlinx-coroutines-core-1.7.3-sources.jar"), "kotlinx-coroutines-core-jvm"),
        )
    }

    @Test
    @DisplayName("KDoc is extracted for generic + annotated functions")
    fun testKdocGenericAnnotatedFunction() {
        val source = """
            package androidx.compose.runtime
            /**
             * Remember the value produced by calculation.
             */
            @Composable
            public inline fun <T> remember(crossinline calculation: () -> T): T = TODO()

            /** Load a string resource. */
            @Composable
            @ReadOnlyComposable
            fun stringResource(id: Int): String = TODO()
        """.trimIndent()
        val map = SourcesKdocReader.extractKdocFromSource(source)
        assertTrue(
            map["remember"]?.startsWith("Remember the value") == true,
            "generic `fun <T> remember` KDoc must be captured; got: ${map["remember"]}",
        )
        assertTrue(
            map["stringResource"]?.startsWith("Load a string") == true,
            "annotated `fun stringResource` KDoc must be captured; got: ${map["stringResource"]}",
        )
    }

    @Test
    @DisplayName("KDoc survives multi-line annotations (e.g. @Composable)")
    fun testKdocMultilineAnnotation() {
        val source = """
            package androidx.compose.runtime
            /**
             * Functions and the values they produce can be marked as Composable.
             */
            @MustBeDocumented
            @Retention(AnnotationRetention.BINARY)
            @Target(
                // function declarations
                // @Composable fun Foo() { ... }
                AnnotationTarget.FUNCTION,
                // type usages: foo: @Composable () -> Unit
                AnnotationTarget.TYPE,
                AnnotationTarget.PROPERTY_GETTER,
            )
            public annotation class Composable
        """.trimIndent()
        val map = SourcesKdocReader.extractKdocFromSource(source)
        assertTrue(
            map["Composable"]?.startsWith("Functions and the values") == true,
            "KDoc before a multi-line @Target annotation (with comments containing parens) must be captured; got: ${map["Composable"]}",
        )
    }

    @Test
    @DisplayName("stripNonKdocComments preserves KDoc URLs, drops other comments")
    fun testStripNonKdocComments() {
        val src = "/** see https://example.com */\nfun f() {} // trailing\n/* block */ val x = 1"
        val out = SourcesKdocReader.stripNonKdocComments(src)
        assertTrue(out.contains("https://example.com"), "KDoc URL must survive: $out")
        assertTrue(!out.contains("trailing"), "line comment dropped: $out")
        assertTrue(!out.contains("block"), "block comment dropped: $out")
    }

    @Test
    @DisplayName("indexJarFile returns empty list for corrupted JAR")
    fun testCorruptedJar(@TempDir tmpDir: File) {
        val jarFile = File(tmpDir, "corrupt.jar")
        jarFile.writeBytes(notAZipArchive())

        val result = indexJarFile(jarFile.absolutePath)
        assertTrue(result.isEmpty(), "should return empty for corrupted JAR")
    }

    @Test
    @DisplayName("indexJarFile returns empty list for JAR with no .class entries")
    fun testEmptyJar(@TempDir tmpDir: File) {
        val jarFile = createTestJar(tmpDir, "empty.jar", emptyMap())
        val result = indexJarFile(jarFile.absolutePath)
        assertTrue(result.isEmpty(), "should return empty for JAR with no .class files")
    }

    @Test
    @DisplayName("indexJarFile handles .aar with classes.jar inside")
    fun testAar(@TempDir tmpDir: File) {
        val classBytes = minimalClassBytes("com/example/AarClass")
        val classesJarBytes = java.io.ByteArrayOutputStream().also { baos ->
            JarOutputStream(baos).use { jos ->
                jos.putNextEntry(ZipEntry("com/example/AarClass.class"))
                jos.write(classBytes)
                jos.closeEntry()
            }
        }.toByteArray()

        val aarFile = createTestJar(tmpDir, "test.aar", mapOf("classes.jar" to classesJarBytes))
        val result = indexJarFile(aarFile.absolutePath)

        assertTrue(result.isNotEmpty(), "should index symbols from AAR")
        assertTrue(result.any { it.name == "AarClass" },
            "should find AarClass; got: ${result.map { it.name }}")
    }

    @Test
    @DisplayName("indexClassBytes indexes a real named nested (\$-named, pure-Java) class")
    fun testJavaNestedClassIsIndexed(@TempDir tmpDir: File) {
        // A real named nested class (`Outer$Inner`, referenced from Kotlin as
        // `Outer.Inner`) is a legitimate, user-nameable declaration — same as
        // a real top-level class — and must be indexed under its own bare
        // name with its enclosing class as its container.
        val innerBytes = minimalClassBytes("com/example/Outer\$Inner")
        val result = indexClassBytes(innerBytes)
        val cls = result.singleOrNull { it.name == "Inner" && it.kind == "class" }
        assertTrue(cls != null, "should index the real named nested class; got: ${result.map { it.name }}")
        assertEquals("Outer", cls!!.container, "nested class's container must be its enclosing class")
        assertFalse(cls.topLevel, "a nested class is not top-level")
    }

    @Test
    @DisplayName("indexClassBytes skips an anonymous/local (compiler-numbered) nested class")
    fun testJavaAnonymousNestedClassIsExcluded(@TempDir tmpDir: File) {
        // javac numbers anonymous and local classes (`Outer$1`, `Outer$1Local`)
        // — never true for a class with a real declared name — so this shape
        // must stay excluded even though it's otherwise ACC_PUBLIC.
        val anonymousBytes = minimalClassBytes("com/example/Outer\$1")
        val result = indexClassBytes(anonymousBytes)
        assertTrue(result.isEmpty(), "should skip an anonymous (numbered) nested class; got: ${result.map { it.name }}")
    }

    @Test
    @DisplayName("indexClassBytes handles non-public class")
    fun testNonPublicClass(@TempDir tmpDir: File) {
        val cw = org.objectweb.asm.ClassWriter(0)
        cw.visit(
            org.objectweb.asm.Opcodes.V1_8,
            0, // no ACC_PUBLIC
            "com/example/PackagePrivate",
            null,
            "java/lang/Object",
            null
        )
        cw.visitEnd()

        val result = indexClassBytes(cw.toByteArray())
        assertTrue(result.isEmpty(), "should skip non-public class")
    }

    @Test
    @DisplayName("SourcesKdocReader.findSourcesJar returns null for nonexistent path")
    fun testFindSourcesJarNonexistent() {
        val result = SourcesKdocReader.readKdocMap("/nonexistent/path/foo.jar")
        assertTrue(result.isEmpty(), "should return empty for nonexistent path")
    }

    @Test
    @DisplayName("SourcesKdocReader.extractKdocFromSource finds KDoc comments")
    fun testKdocExtraction() {
        val source = """
            /** A test function. */
            fun testFunc() = 42

            /** Another function
             * with multiple lines.
             */
            fun otherFunc(x: Int): String = "${'$'}x"
        """.trimIndent()

        val result = SourcesKdocReader.extractKdocFromSource(source)

        assertTrue(result.containsKey("testFunc"), "should find testFunc; got keys: ${result.keys}")
        assertEquals("A test function.", result["testFunc"])
        assertTrue(result.containsKey("otherFunc"), "should find otherFunc")
    }

    private fun notAZipArchive(): ByteArray = "this is not a zip file".toByteArray()

    @Test
    @DisplayName("indexJarFile flags @Deprecated guidance overloads of launch")
    fun testDeprecatedGuidanceOverloadsAreFlagged() {
        // kotlinx-coroutines 1.11.0 ships @Deprecated "guidance" overloads in
        // Guidance.kt that exist purely to surface a compile error when you pass a
        // Job / NonCancellable where a CoroutineContext is expected:
        //   @Deprecated(...) fun CoroutineScope.launch(context: Job, ...): Job
        //   @Deprecated(...) fun CoroutineScope.launch(context: NonCancellable, ...): Job
        // The real overload `launch(context: CoroutineContext, ...)` is NOT deprecated.
        // Without deprecation capture these three all leak into completion as
        // separate `launch` suggestions.
        val jarPath = System.getProperty("coroutines.jar")
        assertNotNull(jarPath, "coroutines.jar system property must be set by the build")

        val launches = indexJarFile(jarPath!!)
            .filter { it.name == "launch" && it.extensionReceiverType == "CoroutineScope" }
        assertTrue(
            launches.size >= 3,
            "expected the real + two guidance launch overloads; got: ${launches.map { it.detail }}",
        )

        val real = launches.filter { it.detail.contains("context: CoroutineContext") }
        val guidance = launches.filter {
            it.detail.contains("context: Job") || it.detail.contains("context: NonCancellable")
        }

        assertTrue(real.isNotEmpty(), "real launch(context: CoroutineContext) overload missing")
        assertTrue(
            real.none { it.deprecated },
            "real launch(context: CoroutineContext) must NOT be flagged deprecated",
        )
        assertEquals(
            2, guidance.size,
            "expected exactly the Job + NonCancellable guidance overloads; got: ${guidance.map { it.detail }}",
        )
        assertTrue(
            guidance.all { it.deprecated },
            "guidance launch(Job)/launch(NonCancellable) overloads must be flagged deprecated; " +
                "got: ${guidance.map { "${it.detail} -> deprecated=${it.deprecated}" }}",
        )
    }

    @Test
    @DisplayName("indexJarFile marks defaulted parameters in rendered function detail")
    fun testDefaultedParametersAreMarkedInDetail() {
        // The real `launch` overload's `context`/`start` parameters both carry
        // Kotlin default values (`= EmptyCoroutineContext`, `= CoroutineStart.DEFAULT`);
        // only `block` is truly required. Without a default-value marker in the
        // rendered detail text, the Rust side's `params_from_detail` (which infers
        // "required" purely from the ABSENCE of `=` in each parameter's own text)
        // has no way to tell these apart from a fully-required 3-arg function --
        // causing a real call like `scope.launch { ... }` (0 explicit args, only a
        // trailing lambda) to fail arity/shape filtering against a wrongly-required
        // `context`/`start`, and get treated as unresolved/ambiguous. This is a
        // real, measured bug found via the resolution-accuracy benchmark on a real
        // Kotlin/Android corpus (Moneta) -- see the 2026-08-26 investigation.
        val jarPath = System.getProperty("coroutines.jar")
        assertNotNull(jarPath, "coroutines.jar system property must be set by the build")

        val real = indexJarFile(jarPath!!)
            .filter { it.name == "launch" && it.extensionReceiverType == "CoroutineScope" }
            .firstOrNull { it.detail.contains("context: CoroutineContext") && !it.deprecated }
        assertNotNull(real, "real launch(context: CoroutineContext, ...) overload missing")

        assertTrue(
            real!!.detail.contains("context: CoroutineContext =") &&
                real.detail.contains("start: CoroutineStart ="),
            "context/start both declare Kotlin default values and must be marked " +
                "with '=' in the rendered detail so params_from_detail counts them " +
                "as optional, got: ${real.detail}",
        )
        assertFalse(
            real.detail.substringAfter("block:").contains("="),
            "block has no default value and must NOT be marked with '=', " +
                "got: ${real.detail}",
        )
    }

    @Test
    @DisplayName("indexJarFile indexes a JAR-compiled, pure-Java, non-companion nested class (Moshi.Builder)")
    fun testJarCompiledPlainNestedClassIsIndexed() {
        // Moshi's core module is pure Java (no Kotlin metadata at all) — its
        // entire builder API lives in `Moshi$Builder`, a plain static nested
        // class, not a companion (Java has no such concept). The nested-class
        // gate used to exclude EVERY `$`-named class outright in the Java
        // fallback path, so `Moshi.Builder(...)` — a very common real-world
        // constructor call — resolved to zero candidates.
        val jarPath = System.getProperty("moshi.jar")
        assertNotNull(jarPath, "moshi.jar system property must be set by the build")

        val entries = indexJarFile(jarPath!!)

        val builder = entries.singleOrNull { it.name == "Builder" && it.kind == "class" && it.container == "Moshi" }
        assertNotNull(builder, "expected Moshi\$Builder to be indexed under container Moshi; got: " +
            entries.filter { it.name == "Builder" }.map { "${it.name}:${it.container}" })

        val buildMethod = entries.filter { it.name == "build" && it.container == "Builder" }
        assertTrue(buildMethod.isNotEmpty(), "expected Builder.build() to be indexed under container Builder")

        // A public, differently-named nested class in a different top-level
        // class must also work — proves the container-parsing isn't hardcoded
        // to a single class.
        val factory = entries.singleOrNull { it.name == "Factory" && it.kind == "class" && it.container == "JsonAdapter" }
        assertNotNull(factory, "expected JsonAdapter\$Factory to be indexed under container JsonAdapter")
    }

    @Test
    @DisplayName("indexJarFile excludes compiler-synthesized nested classes even with the broadened gate")
    fun testJarSyntheticNestedClassesAreExcluded() {
        // The nested-class gate now admits real named declarations (see
        // testJarCompiledPlainNestedClassIsIndexed / testNamedCompanionObjectMembersAreIndexed),
        // so this regression-tests that the compiler's own synthetic helpers
        // (`WhenMappings`, `DefaultImpls`, lambdas, anonymous objects) stay
        // excluded — none of them are declarations a Kotlin caller could ever
        // reference by name.
        val jarPath = System.getProperty("coroutines.jar")
        assertNotNull(jarPath, "coroutines.jar system property must be set by the build")

        val entries = indexJarFile(jarPath!!)

        assertTrue(
            entries.none { it.container == "CoroutineStart" && it.name == "WhenMappings" },
            "WhenMappings must never be indexed as a nested class",
        )
        assertTrue(
            entries.none { it.name == "DefaultImpls" },
            "DefaultImpls must never be indexed as a nested class",
        )
        assertTrue(
            entries.none { it.name.contains('$') },
            "no indexed symbol name should ever contain a raw '\$' " +
                "(covers anonymous lambda/object-expression classes like AwaitKt\$joinAll\$1 " +
                "and CoroutineExceptionHandlerKt\$CoroutineExceptionHandler\$1); got: " +
                entries.filter { it.name.contains('$') }.map { "${it.name}:${it.container}" },
        )
    }

    @Test
    @DisplayName("indexJarFile finds Timber's named companion object (Forest) and its members")
    fun testNamedCompanionObjectMembersAreIndexed() {
        // Timber's entire public API (`d`, `e`, `i`, `w`, `tag`, `plant`, ...)
        // lives inside `companion object Forest : Tree()` -- a NAMED
        // companion, compiled as `Timber$Forest`, not the default unnamed
        // `Timber$Companion` shape the indexer used to special-case by name
        // suffix alone. Real, measured regression: this entire API was
        // previously unindexed (only the bare `Timber` class itself and an
        // unrelated, empty `Timber$DebugTree$Companion` survived), which in
        // turn made every `Timber.d(...)`/`Timber.e(...)` call in a real
        // consuming project resolve to zero candidates.
        val jarPath = System.getProperty("timber.jar")
        assertNotNull(jarPath, "timber.jar system property must be set by the build")

        val entries = indexJarFile(jarPath!!)

        val timberClass = entries.singleOrNull { it.name == "Timber" && it.kind == "class" }
        assertNotNull(timberClass, "expected the Timber class itself to be indexed")

        val forest = entries.singleOrNull { it.name == "Forest" && it.kind == "object" }
        assertNotNull(forest, "expected the named Forest companion to be indexed")
        assertEquals(
            "Timber", forest!!.container,
            "Forest's own container must be Timber (its enclosing class), " +
                "not empty and not a fully-qualified 'Timber.Forest'",
        )
        assertTrue(
            forest.detail.startsWith("companion object"),
            "Forest's detail must say 'companion object', the signal " +
                "`is_companion_object()` on the Rust side keys off of, got: ${forest.detail}",
        )

        val logMethodNames = setOf("d", "e", "i", "w", "v", "wtf", "log", "tag", "plant", "uproot")
        val logMethods = entries.filter { it.name in logMethodNames && it.container == "Forest" }
        assertTrue(
            logMethods.map { it.name }.toSet().containsAll(logMethodNames),
            "expected all of Timber's log methods under Forest's own container, " +
                "got: ${logMethods.map { it.name }}",
        )
    }

    @Test
    @DisplayName("indexJarFile indexes a compiled enum class's own constants")
    fun testCompiledEnumClassConstantsAreIndexed() {
        // `entriesFromClass` iterates `klass.functions`/`klass.properties` but
        // never `klass.enumEntries` -- so a JAR-compiled enum class's own
        // constants (e.g. `BufferOverflow.DROP_OLDEST`) were never indexed at
        // all, even though the enum class itself (and any real functions/
        // properties it declares) were. Real, measured gap: any qualified
        // reference to a JAR-compiled enum constant resolves to zero
        // candidates in a real consuming project.
        val jarPath = System.getProperty("coroutines.jar")
        assertNotNull(jarPath, "coroutines.jar system property must be set by the build")

        val entries = indexJarFile(jarPath!!)

        val bufferOverflow = entries.singleOrNull { it.name == "BufferOverflow" && it.kind == "class" }
        assertNotNull(bufferOverflow, "expected the BufferOverflow enum class itself to be indexed")

        val constantNames = setOf("SUSPEND", "DROP_OLDEST", "DROP_LATEST")
        val constants = entries.filter { it.name in constantNames && it.container == "BufferOverflow" }
        assertEquals(
            constantNames,
            constants.map { it.name }.toSet(),
            "expected all three BufferOverflow enum constants under its own container, " +
                "got: ${entries.filter { it.container == "BufferOverflow" }.map { "${it.name}:${it.kind}" }}",
        )
        assertTrue(
            constants.all { it.kind == "enum_member" },
            "enum constants must carry the 'enum_member' kind so the Rust side maps them " +
                "to SymbolKind::ENUM_MEMBER (matching source-parsed enum constants), " +
                "got: ${constants.map { "${it.name}:${it.kind}" }}",
        )
    }
}
