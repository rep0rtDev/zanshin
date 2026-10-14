plugins {
    kotlin("jvm") version "2.0.21"
    `java-library`
}

group = "dev.zanshin"
version = "0.1.0"

repositories { mavenCentral() }

kotlin { jvmToolchain(11) }

dependencies {
    testImplementation(kotlin("test"))
}

tasks.test { useJUnitPlatform() }
