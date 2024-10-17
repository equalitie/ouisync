# Ouisync Kotlin bindings example app

This is an example app that shows basic usage of the Ouisync library. It's a simple[Jetpack Compose]
(https://developer.android.com/compose) app. It's structured as follows:

The `ViewModel` demonstates how to initialize Ouisync `Session`, how to configure it, handle errors
and how to close it when the app exists. Additionally it shows how to create new repository
(including importing it with a share token), opening existing repositories from disk, closing
repositories and deleting them.

The UI iself consist of three screens:

`RepositoryListScreen` is for managing the repositories. It shows how to open/create/import
repositories, how to share repositories using Android Sharesheet and how to delete repositories.

`FolderScreen` shows the content of a given folder in a given repository and allows to navigate to
subfolders and files. It also shows how to show a "live" view of a folder which automatically
refreshes whenever the repository gets updated by a peer.

Finally `FileScreen` shows how to open a file, monitor it's sync progress and how to read it's
content. Similarry to the `FolderScreen`, it also shows how to automatically refresh the screen
when the file gets updated.

For more andvanced usage, refer to the API docs (TODO: link).