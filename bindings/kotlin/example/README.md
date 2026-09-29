# Ouisync Kotlin bindings example app

This is an example app that shows basic usage of the Ouisync library. It's a simple
[Compose Multiplatform](https://www.jetbrains.com/compose-multiplatform/) app which runs on Android
and on desktop (Linux, macOS, Windows). It consists of the following modules:

- `shared`: Kotlin Multiplatform library with the UI and the logic of the app. Almost all the code
  is in `commonMain`. Only the few platform specific bits (logging and sharing) are in `androidMain`
  and `jvmMain`. It depends on `ouisync-session` and `ouisync-service-common`.
- `android`: The Android app. Contains only the `MainActivity` and depends on
  `ouisync-service-android` which provides the native library for Android.
- `desktop`: The desktop app. Contains only the `main` function and depends on
  `ouisync-service-jvm` which provides the native library for desktop platforms.

To build and run the Android app, run `./gradlew example:android:assembleDebug` (from the
`bindings/kotlin` directory), then install the apk from
`build/example/android/outputs/apk/debug/example-android-debug.apk` on a device or an emulator.

To run the desktop app, run `./gradlew example:desktop:run`. The app stores its data in
`~/.ouisync-kotlin-example` by default. A different directory can be passed as the first command
line argument, e.g. `./gradlew example:desktop:run --args=/path/to/dir` (useful for running
multiple instances on the same machine to test syncing between them).

The `ViewModel` demonstrates how to initialize Ouisync `Session`, how to configure it, handle errors
and how to close it when the app exists. Additionally it shows how to create new repository
(including importing it with a share token), opening existing repositories from disk, closing
repositories and deleting them.

The UI itself consist of three screens:

`RepositoryListScreen` is for managing the repositories. It shows how to open/create/import
repositories, how to share repositories (using the Android Sharesheet on Android or the clipboard
on desktop) and how to delete repositories.

`FolderScreen` shows the content of a given folder in a given repository and allows to navigate to
subfolders and files. It also shows how to show a "live" view of a folder which automatically
refreshes whenever the repository gets updated by a peer.

Finally `FileScreen` shows how to open a file, monitor it's sync progress and how to read it's
content. Similarly to the `FolderScreen`, it also shows how to automatically refresh the screen
when the file gets updated.

For more advanced usage, refer to the [API docs](https://docs.ouisync.net/kotlin/)