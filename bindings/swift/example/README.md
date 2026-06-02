# OuisyncExample

A minimal SwiftUI example app for the OuisyncLib Swift bindings. The single
Xcode project targets both **macOS** (14+) and **iOS** (17+) from one shared
codebase.

## Structure

`ExampleViewModel` manages the service and session lifecycle and exposes
repository state to the UI via `@Published` properties. The UI is split across
three views:

| File | Screen | Demonstrates |
|---|---|---|
| `RepositoryListView.swift` | Repository list | Creating, deleting, and sharing repositories |
| `FolderView.swift` | Folder contents | Browsing directories recursively |
| `FileView.swift` | File detail | Opening a file, tracking sync progress, reading content |

Platform differences (clipboard API, window sizing) are isolated to a handful
of `#if os(macOS)` guards in those files.

## Prerequisites

Build the xcframework before opening the project (from the repo root):

```sh
bash bindings/swift/OuisyncLib/build-xcframework.sh
```

This compiles the Rust service for all targets in `config.sh` and produces
`OuisyncLib/output/OuisyncLibFFI.xcframework`.

## Running

Open the project in Xcode:

```sh
open bindings/swift/example/OuisyncExample.xcodeproj
```

Select a destination (Mac, iPhone simulator, iPad simulator) from the scheme
picker and press **Run** (⌘R).

The app stores its data under `~/Library/Application Support/OuisyncExample/`.

## Usage

The app starts the Ouisync service automatically on launch.

**Repository list** — lists all repositories. Use the **+** button to create a
new one (optionally pasting a share token to import someone else's repository).
The **share** icon copies a write-access share token to the clipboard; the
**trash** icon deletes the repository.

**Folder view** — tap a repository to browse its root directory. Subdirectories
and files are shown; tap to navigate deeper.

**File view** — shows file size, sync status, and SHA-256 hash. Use the pencil
icon to write text content and the refresh button to re-read after the file
syncs further.

> **Note:** The Swift bindings do not yet expose a `subscribe()` method on
> `Repository`, so the file view polls for sync progress (once per second)
> rather than reacting to live notifications.
