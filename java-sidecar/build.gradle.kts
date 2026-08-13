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
            // Opt-in static/musl build (test workflow only — never set for the real
            // release matrix). Produces a linker-independent binary that runs under
            // environments without a standard glibc dynamic linker, e.g. Termux.
            // -PmuslZlibDir and -PmuslCC point at the toolchain the test workflow sets up.
            if (project.hasProperty("staticMusl")) {
                buildArgs.addAll("--static", "--libc=musl")
                (project.findProperty("muslZlibDir") as String?)?.let {
                    buildArgs.addAll("-H:NativeLinkerOption=-L$it", "-H:NativeLinkerOption=-lz")
                }
                (project.findProperty("muslCC") as String?)?.let {
                    buildArgs.add("-H:CCompilerPath=$it")
                }
            }
        }
    }
    toolchainDetection.set(false)
}
