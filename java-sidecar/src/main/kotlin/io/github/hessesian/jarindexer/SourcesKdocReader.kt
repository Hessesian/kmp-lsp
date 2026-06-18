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

        val candidates = versionDir.walkTopDown()
            .filter { it.isFile && it.name.endsWith("-sources.jar", ignoreCase = true) }
            .toList()
        val chosen = selectSourcesJar(candidates.map { it.name }, File(mainJarPath).nameWithoutExtension)
            ?: return null
        return candidates.firstOrNull { it.name == chosen }
    }

    /**
     * Pick the best `-sources.jar` for a main artifact among `candidateNames`.
     *
     * Many AARs ship both the real API sources (`ui-android-1.11.2-sources.jar`) and a
     * **samples** jar (`ui-1.11.2-samples-sources.jar`) that contains usage examples, not
     * the documented declarations. The old code took the first match and frequently grabbed
     * the samples jar → empty KDoc for `stringResource`, `remember`, etc. Exclude samples and
     * prefer the jar whose name matches the main artifact stem.
     */
    fun selectSourcesJar(candidateNames: List<String>, mainStem: String): String? {
        val real = candidateNames.filterNot {
            it.endsWith("-samples-sources.jar", ignoreCase = true)
        }
        if (real.isEmpty()) return null
        val norm = mainStem.removeSuffix("-jvm").removeSuffix("-android")
        return real.firstOrNull { it.startsWith(mainStem) }
            ?: real.firstOrNull { it.startsWith(norm) }
            ?: real.first()
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
        """/\*\*(.*?)\*/\s*(?:@[\w.:]+\s*(?:\([^)]*\))?\s*)*(?:(?:public|internal|protected|private|actual|expect|inline|suspend|override|operator|infix|tailrec|external|open|abstract|sealed|data|enum|annotation|inner|companion|value|const|lateinit|var|val)\s+)*(?:fun|class|interface|object|typealias|val|var)\s+(?:<[^>]*>\s*)?(?:\w[\w.<>, ?]*\.)?\s*(\w+)""",
        setOf(RegexOption.DOT_MATCHES_ALL, RegexOption.MULTILINE)
    )

    fun extractKdocFromSource(source: String): Map<String, String> {
        val result = mutableMapOf<String, String>()
        // Strip line/block comments (but keep KDoc) so that comments containing `)`
        // inside a multi-line annotation — e.g. `@Target( // @Composable fun Foo() ... )`
        // before `annotation class Composable` — don't truncate the paren match.
        for (match in KDOC_DECL_PATTERN.findAll(stripNonKdocComments(source))) {
            val rawDoc = match.groupValues[1]
            val name = match.groupValues[2]
            if (name.isNotEmpty()) {
                result[name] = cleanKdoc(rawDoc)
            }
        }
        return result
    }

    /** Remove `//` line and `/* */` block comments while preserving `/** */` KDoc. */
    fun stripNonKdocComments(source: String): String {
        val sb = StringBuilder(source.length)
        var i = 0
        val n = source.length
        while (i < n) {
            val c = source[i]
            val next = if (i + 1 < n) source[i + 1] else ' '
            when {
                c == '/' && next == '*' -> {
                    val isKdoc = i + 2 < n && source[i + 2] == '*' &&
                        !(i + 3 < n && source[i + 3] == '/')
                    val end = source.indexOf("*/", i + 2)
                    if (end < 0) {
                        if (isKdoc) sb.append(source, i, n)
                        i = n
                    } else {
                        if (isKdoc) sb.append(source, i, end + 2) else sb.append(' ')
                        i = end + 2
                    }
                }
                c == '/' && next == '/' -> {
                    val end = source.indexOf('\n', i)
                    if (end < 0) {
                        i = n
                    } else {
                        sb.append('\n')
                        i = end + 1
                    }
                }
                else -> {
                    sb.append(c)
                    i++
                }
            }
        }
        return sb.toString()
    }

    /** Strip leading `*` and whitespace from each KDoc line. */
    private fun cleanKdoc(raw: String): String =
        raw.lines()
            .map { it.trim().removePrefix("*").trim() }
            .dropWhile { it.isEmpty() }
            .dropLastWhile { it.isEmpty() }
            .joinToString("\n")
}
