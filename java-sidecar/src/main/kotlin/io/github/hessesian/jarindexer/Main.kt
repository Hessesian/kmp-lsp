package io.github.hessesian.jarindexer

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

import java.io.BufferedReader
import java.io.InputStreamReader

@Serializable
private data class Request(val jar: String? = null, val shutdown: Boolean = false)

private val json = Json { ignoreUnknownKeys = true }

fun main() {
    val reader = BufferedReader(InputStreamReader(System.`in`))
    var line = reader.readLine()
    while (line != null) {
        val req = runCatching { json.decodeFromString<Request>(line) }.getOrNull()
        if (req == null || req.shutdown) break

        val jarPath = req.jar
        if (jarPath != null) {
            val entries = indexJarFile(jarPath)
            val out = json.encodeToString(
                kotlinx.serialization.builtins.ListSerializer(
                    io.github.hessesian.jarindexer.model.SymbolEntry.serializer()
                ),
                entries
            )
            println(out)
            System.out.flush()
        }

        line = reader.readLine()
    }
}
