# Ouisync gradle convention plugin

Gradle convention plugin that applies some common properties to projects:

- Sets the `ndkVersion` property on android applications and android libraries. The version is read
  from a file named `ndk.version` which it looks for in the current project directory and all
  parent directories (uses the the first one found).

