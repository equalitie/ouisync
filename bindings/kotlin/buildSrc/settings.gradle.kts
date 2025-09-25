rootProject.name = "buildSrc"

// pluginManagement {
//     repositories {
//         gradlePluginPortal()
//     }
// }

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)

    repositories {
        google()
        gradlePluginPortal()
        mavenCentral()
    }
}