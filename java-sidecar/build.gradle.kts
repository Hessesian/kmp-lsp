plugins {
    kotlin("jvm") version "2.0.21"
    kotlin("plugin.serialization") version "2.0.21"
    id("com.github.johnrengelman.shadow") version "8.1.1"
    application
    id("org.graalvm.buildtools.native") version "0.10.4"
}

group = "io.github.hessesian"
version = "1.0.0"

repositories {
    mavenCentral()
}

dependencies {
    // Kotlin metadata: decode @kotlin.Metadata → true Kotlin signatures
    implementation("org.jetbrains.kotlinx:kotlinx-metadata-jvm:0.9.0")
    // ASM: read .class annotation bytes without loading classes into JVM
    implementation("org.ow2.asm:asm:9.7.1")
    // JSON I/O
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
    // Tests
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.3")
}

// Real-world fixture JAR for the deprecation regression test: coroutines 1.11.0
// ships @Deprecated "guidance" overloads of `launch` (e.g. `launch(context: Job)`).
// Resolved into its own configuration (non-transitive) so its newer kotlin-stdlib
// never lands on the compile classpath — the test only needs the jar file, whose
// path is handed to it via the `coroutines.jar` system property below.
val coroutinesFixture by configurations.creating { isTransitive = false }
dependencies {
    coroutinesFixture("org.jetbrains.kotlinx:kotlinx-coroutines-core-jvm:1.11.0")
}

// Real-world fixture JAR for the named-companion-object regression test:
// Timber's entire public API (`d`, `e`, `i`, `w`, `tag`, `plant`, ...) lives in
// `companion object Forest : Tree()` — a NAMED companion, compiled as
// `Timber$Forest`, not the default-companion `Timber$Companion` shape the
// indexer's `$`-name filter already special-cased.
val timberFixture by configurations.creating { isTransitive = false }
dependencies {
    timberFixture("com.jakewharton.timber:timber:5.0.1")
}

// Real-world fixture JAR for the plain-nested-class regression test: Moshi's
// core module is pure Java (no Kotlin metadata), and its entire builder API
// lives in `Moshi$Builder` — a plain static nested class, not a companion
// (Java has no such concept). Also covers a differently-named nested class
// in a different class (`JsonAdapter$Factory`).
val moshiFixture by configurations.creating { isTransitive = false }
dependencies {
    moshiFixture("com.squareup.moshi:moshi:1.15.2")
}

application {
    mainClass.set("io.github.hessesian.jarindexer.MainKt")
}

kotlin {
    jvmToolchain(21)
}

tasks.test {
    useJUnitPlatform()
    // Hand the fixture jar path to the deprecation regression test.
    doFirst {
        coroutinesFixture.files
            .firstOrNull { it.name.startsWith("kotlinx-coroutines-core-jvm") }
            ?.let { systemProperty("coroutines.jar", it.absolutePath) }
        timberFixture.files
            .firstOrNull { it.name.startsWith("timber") }
            ?.let { systemProperty("timber.jar", it.absolutePath) }
        moshiFixture.files
            .firstOrNull { it.name.startsWith("moshi") }
            ?.let { systemProperty("moshi.jar", it.absolutePath) }
    }
}

tasks.shadowJar {
    archiveClassifier.set("")
    archiveVersion.set("")
    mergeServiceFiles()
}

// GraalVM native-image configuration
graalvmNative {
    binaries {
        named("main") {
            imageName.set("kmp-jar-indexer")
            mainClass.set("io.github.hessesian.jarindexer.MainKt")
            buildArgs.addAll(
                "--no-fallback",
                "--initialize-at-build-time=kotlin",
                "-H:+ReportExceptionStackTraces",
                // Reduce binary size
                "--gc=serial",
                "-O2",
            )
        }
    }
    toolchainDetection.set(false)
}
