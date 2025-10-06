plugins {
    `kotlin-dsl`
}

gradlePlugin {
    plugins {
        create("ouisync") {
            id = "org.equalitie.ouisync.convention"
            implementationClass = "org.equalitie.ouisync.ConventionPlugin"
        }
    }
}

dependencies {
    compileOnly("com.android.tools.build:gradle:8.10.0")
}
