plugins { kotlin("jvm"); `maven-publish` }
kotlin { jvmToolchain(17) }
val nativeHost = (if (System.getProperty("os.name").startsWith("Mac")) "darwin" else "linux") + "-" +
    (if (System.getProperty("os.arch") in listOf("aarch64", "arm64")) "arm64" else "x86_64")
tasks.jar { archiveBaseName.set("bark-bolt-ffi-jvm-$nativeHost"); archiveVersion.set(project.version.toString()) }
sourceSets {
    main { kotlin.srcDir(rootProject.file("dist/kotlin")); kotlin.srcDir(rootProject.file("sdk/kotlin-support")); resources.srcDir(rootProject.file("dist/android/desktopJniLibs")) }
    test { kotlin.srcDir(rootProject.file("tests/kotlin")) }
}
dependencies {
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
    testImplementation(kotlin("test-junit"))
}
tasks.test {
    inputs.property("regtest", System.getenv("BARK_REGTEST") ?: "")
    inputs.dir(rootProject.file("tests/fixtures"))
    inputs.dir(rootProject.file("tests/support"))
    systemProperty("bark.fixtures", rootProject.file("tests/fixtures").absolutePath)
    environment("BARK_REGTEST", System.getenv("BARK_REGTEST") ?: "")
}
publishing { publications { create<MavenPublication>("jvm") {
    from(components["java"])
    groupId = project.group.toString(); artifactId = "bark-bolt-ffi-jvm-$nativeHost"; version = project.version.toString()
} } }

tasks.register<JavaExec>("testPackaged") {
    dependsOn(tasks.testClasses, tasks.jar)
    classpath = files(tasks.jar) + (sourceSets.test.get().runtimeClasspath - sourceSets.main.get().output)
    mainClass.set("org.junit.runner.JUnitCore")
    args("ContractTest", "NoahContractTest", "NoahP1ContractTest", "V030ContractTest", "V070ContractTest", "FullBalanceContractTest", "PaymentRequestContractTest")
    systemProperty("bark.fixtures", rootProject.file("tests/fixtures").absolutePath)
    environment("BARK_REGTEST", System.getenv("BARK_REGTEST") ?: "")
}
