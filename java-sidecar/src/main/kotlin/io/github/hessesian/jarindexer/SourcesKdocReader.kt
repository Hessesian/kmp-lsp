package io.github.hessesian.jarindexer

import java.io.ByteArrayInputStream
import java.io.File
import java.util.zip.ZipInputStream

/**
 * Extracts KDoc comments from a `-sources.jar` file.
 *
 * Gradle stores sources JARs next to the main JAR under a sibling hash directory
 * in `modules-2/files-2.1/<group>/<artifact>/<version>/`.
 * We walk up to the version directory and search all child dirs for a `*-sources.jar`.
 *
 * Returns a map of `simpleName -> KDoc text` (without the `/** ... */` delimiters).
 */
object SourcesKdocReader {

    /** Find and parse the sources JAR for a given main JAR path. */
    fun readKdocMap(mainJarPath: String): Map<String, String> {
        val sourcesJar = findSourcesJar(mainJarPath) ?: return emptyMap()
        return try {
            extractKdocFromSourcesJar(sourcesJar)
        } catch (_: Exception) {
            emptyMap()
        }
    }

    private fun findSourcesJar(mainJarPath: String): File? {
        // Walk up to the version directory (4 levels up from the JAR file:
        // .../files-2.1/<group>/<artifact>/<version>/<hash>/<file>.jar)
        val versionDir = File(mainJarPath).parentFile?.parentFile ?: return null
        if (!versionDir.isDirectory) return null

        val artifactName = File(mainJarPath).nameWithoutExtension
            .removeSuffix("-jvm")  // kotlinx-coroutines-core-jvm-1.7.3 → kotlinx-coroutines-core
            .substringBeforeLast("-")  // strip version suffix

        return versionDir.walkTopDown()
            .filter { it.isFile && it.name.endsWith("-sources.jar", ignoreCase = true) }
            .firstOrNull()
    }

    /**
     * Parse all `.kt` source files in the sources JAR and extract top-level KDoc
     * comments mapped to the immediately following declaration name.
     *
     * Pattern: `/** ... */` block immediately preceding `fun`, `class`, `object`,
     * `interface`, `val`, `var`, or `typealias`.
     */
    private fun extractKdocFromSourcesJar(sourcesJar: File): Map<String, String> {
        val result = mutableMapOf<String, String>()
        val bytes = sourcesJar.readBytes()
        ZipInputStream(ByteArrayInputStream(bytes)).use { zip ->
            var entry = zip.nextEntry
            while (entry != null) {
                if (!entry.isDirectory && entry.name.endsWith(".kt", ignoreCase = true)) {
                    val source = zip.readBytes().toString(Charsets.UTF_8)
                    result.putAll(extractKdocFromSource(source))
                }
                entry = zip.nextEntry
            }
        }
        return result
    }

    private val KDOC_DECL_PATTERN = Regex(
        """/\*\*(.*?)\*/\s*(?:@\w[^\n]*\n\s*)*(?:(?:public|internal|protected|private|actual|expect|inline|suspend|override|operator|infix|tailrec|external|open|abstract|sealed|data|enum|annotation|inner|companion|value|const|lateinit|var|val)\s+)*(?:fun|class|interface|object|typealias|val|var)\s+(?:\w[\w.<>, ?]*\.)?\s*(\w+)""",
        setOf(RegexOption.DOT_MATCHES_ALL, RegexOption.MULTILINE)
    )

    fun extractKdocFromSource(source: String): Map<String, String> {
        val result = mutableMapOf<String, String>()
        for (match in KDOC_DECL_PATTERN.findAll(source)) {
            val rawDoc = match.groupValues[1]
            val name = match.groupValues[2]
            if (name.isNotEmpty()) {
                result[name] = cleanKdoc(rawDoc)
            }
        }
        return result
    }

    /** Strip leading `*` and whitespace from each KDoc line. */
    private fun cleanKdoc(raw: String): String =
        raw.lines()
            .map { it.trim().removePrefix("*").trim() }
            .dropWhile { it.isEmpty() }
            .dropLastWhile { it.isEmpty() }
            .joinToString("\n")
}
