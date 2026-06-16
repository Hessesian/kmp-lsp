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
    @DisplayName("indexClassBytes handles class with \$ in name (inner class)")
    fun testInnerClass(@TempDir tmpDir: File) {
        val innerBytes = minimalClassBytes("com/example/Outer\$Inner")
        val result = indexClassBytes(innerBytes)
        // Inner classes with $ should be skipped unless they end with $Companion
        assertTrue(result.isEmpty(), "should skip inner class with \$ in name")
    }

    @Test
    @DisplayName("indexClassBytes accepts Companion classes")
    fun testCompanionClass(@TempDir tmpDir: File) {
        val companionBytes = minimalClassBytes("com/example/Foo\$Companion")
        val result = indexClassBytes(companionBytes)
        // No Kotlin metadata → Java fallback path; ACC_PUBLIC class but name has $
        // JavaClassVisitor skips names containing '$'
        assertTrue(result.isEmpty(), "JavaClassVisitor skips \$ names")
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
}
