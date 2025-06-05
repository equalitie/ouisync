# A Ouisync repository.

## Example usage:

```kotlin
// Create a new repo:
val repo = session.createRepository("path/to/the/repo.ouisyncdb")

// or open an existing one:
val repo = session.openRepository("path/to/the/repo.ouisyncdb")

// Enable syncing with other replicas
repo.setSyncEnabled(true)

// Access the repository files (see File, Directory) ...
val file = repo.openFile("path/to/file")

// Close it when done:
repo.close()
```

## Access repository content

TODO

## Share repository with peers

To share a repository, create the share token with [Repository.share], send it to the peer(e.g., via
a secure instant messenger, encode as QR code and scan, ...), then create a repository on the
peer's device with [State::session_create_repository], passing the share token to it.

## Sync repository with peers

Enable syncing with [Repository.set_sync_enabled]. Afterwards Ouisync will try to automatically find
peers to sync with using various peer discovery methods (Local Discovery, DHT, PEX). Additionally,
peers can be added manually with [Session.add_user_provided_peer].

## Local secrets

Local secrets protect the repository against unauthorized access on the same device and should never
be shared with anyone (to share the repository with peers, use the *share token*). To change the
local secrets, use [Repository.set_access].

## Cache servers

Cache servers relay traffic between peers who can't directly connect to each other. They also
temporarily cache the repository content in order to allow peers to sync even when they are not
online at the same time. See [Repository.create_mirror], [Repository.delete_mirror] and
[Repository.mirror_exists] for more details.

