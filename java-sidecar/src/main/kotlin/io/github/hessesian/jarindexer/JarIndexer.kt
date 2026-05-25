package io.github.hessesian.jarindexer

import io.github.hessesian.jarindexer.model.SymbolEntry
import java.io.ByteArrayInputStream
import java.util.zip.ZipInputStream

/**
 * Index all public symbols from a JAR or AAR file.
 *
 * - JAR: iterate `.class` entries directly
 * - AAR: find `classes.jar` entry inside the outer ZIP, then treat as JAR
 */
fun indexJarFile(path: String): List<SymbolEntry> {
    return try {
        val bytes = java.io.File(path).readBytes()
        when {
            path.endsWith(".aar", ignoreCase = true) -> indexAar(bytes)
            else -> indexJarBytes(bytes)
        }
    } catch (_: Exception) {
        emptyList()
    }
}

private fun indexAar(aarBytes: ByteArray): List<SymbolEntry> {
    ZipInputStream(ByteArrayInputStream(aarBytes)).use { zip ->
        var entry = zip.nextEntry
        while (entry != null) {
            if (entry.name == "classes.jar") {
                val classesJar = zip.readBytes()
                return indexJarBytes(classesJar)
            }
            entry = zip.nextEntry
        }
    }
    return emptyList()
}

private fun indexJarBytes(jarBytes: ByteArray): List<SymbolEntry> {
    val results = mutableListOf<SymbolEntry>()
    ZipInputStream(ByteArrayInputStream(jarBytes)).use { zip ->
        var entry = zip.nextEntry
        while (entry != null) {
            if (!entry.isDirectory && entry.name.endsWith(".class")) {
                val classBytes = zip.readBytes()
                results += indexClassBytes(classBytes)
            }
            entry = zip.nextEntry
        }
    }
    return results
}
