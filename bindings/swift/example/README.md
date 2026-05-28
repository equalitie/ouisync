# Ouisync Swift example app

A minimal macOS SwiftUI app that demonstrates the core Ouisync Swift API. It mirrors the
structure of the [Kotlin example](../../kotlin/example).

## Structure

`ExampleViewModel` manages the service and session lifecycle and exposes repository state
to the UI via `@Published` properties. The UI is split across three views:

| File | Screen | Demonstrates |
|---|---|---|
| `RepositoryListView.swift` | Repository list | Creating, deleting, and sharing repositories |
| `FolderView.swift` | Folder contents | Browsing directories recursively |
| `FileView.swift` | File detail | Opening a file, tracking sync progress, reading content |

## Prerequisites

Build the xcframework before running the example (from the repo root):

```sh
bash bindings/swift/OuisyncLib/build-xcframework-macos.sh
```

Verify it was created:

```sh
ls bindings/swift/OuisyncLib/output/OuisyncLibFFI.xcframework
```

## Running

```sh
cd bindings/swift/example
bash run.sh
```

`run.sh` builds the binary, wraps it in a minimal `.app` bundle, and opens it with
`open(1)` so macOS treats it as a proper GUI app (keyboard focus, Dock icon, etc.).

> **Why not `swift run`?** Running an SPM executable directly from the terminal starts
> the process as a command-line tool. macOS never grants it keyboard focus, so the
> SwiftUI window appears but is unresponsive to typing. Packaging it as a `.app` bundle
> and launching via `open` fixes this.

The app stores its data under `~/Library/Application Support/OuisyncExample/`.

## Usage

The app starts the Ouisync service automatically on launch.

**Repository list** — the main window lists all repositories. Use the **+** button to
create a new one (optionally pasting a share token to import someone else's repository).
The **share** icon copies a write-access share token to the clipboard; the **trash** icon
deletes the repository.

**Folder view** — tap a repository to browse its root directory. Subdirectories and files
are shown; tap to navigate deeper.

**File view** — shows file size, sync status, and SHA-256 hash. Use the **refresh** button
to re-read after the file syncs further.

> **Note:** The Swift bindings do not yet expose a `subscribe()` method on `Repository`,
> so the file view polls for sync progress (once per second) rather than reacting to live
> notifications. This will be fixed when the notification API is wired up.
