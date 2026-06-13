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

application {
    mainClass.set("io.github.hessesian.jarindexer.MainKt")
}

kotlin {
    jvmToolchain(21)
}

tasks.test {
    useJUnitPlatform()
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
