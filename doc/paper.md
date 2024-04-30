# Ouisync

### Overview

Ouisync is a decentralized technology and client application for sharing files and syncing repositories between devices, peer to peer. 
It uses the BitTorrent DHT for addressing and implements encryption of all content transmitted and stored on a user’s device. 
This paper outlines the main components of Ouisync operations and usage, describes the project’s threat model and goes into some detail of 
its technical implementation. For user focused documentation refer to the [FAQ on the Ouisync website](https://ouisync.net/support/).

### Concepts

The core functionality of Ouisync deals with **Repositories**.
A repository is a folder on a local device that is managed by the Ouisync application.The user can add and manipulate files in a Repository, 
imported from the local device to the Ouisync application. A Repository can be and  shared with other peers (also called **Replicas**). A 
repository is created using one of the following Access modes:

- **Blind** mode : peers can sync the Repository but cannot access or modify the files in it.
- **Read** mode: peers can read but not write to the Repository.
- **Write** mode: allow syncing, reading and writing.
  
Each repository is associated with a **Share token** (or just "Token") which is both a globally
unique identifier of the repository and also its access keys. There are three types of tokens
corresponding to each access mode (blind, read and write).

The contents of a repository (files and directories) are stored in a custom format designed for
security and efficient synchronization. The format consists of three parts:

- **Blocks** : encrypted fixed-size chunks the files and directories are split into.
- **Index** : a lookup table to find which blocks belong to which files/directories, also used to determine which blocks belong to the repository.
- **Metadata** : arbitrary key-value pairs associated with the repository but not synced with other
    replicas.

Files and directories are encrypted and split into equally sized blocks to hide the content and any
metadata associated with them.

To support concurrent editing of files within a Repository by two or more peers, the index is divided into **Branches**. Branches are 
further split into **Snapshots** where each snapshot corresponds to a state of the repository at some particular point in time. Within a 
single branch, all snapshots are from a single replica and form a linear history of edits.
Synchronization uses a custom peer-to-peer, end-to-end encrypted protocol which is efficient and secure. Furthermore, the protocol doesn't 
require access to the global internet and is specifically designed to work also in isolated networks and under various adversarial 
conditions.

### Threat model

Ouisync was created for unrestricted and uninhibited data exchange with a focus on users living in highly restricted (and/or censored) 
network environments. For this reason the protocol is decentralized, files and repositories are encrypted in transit and at-rest. However, 
no system is fullproof in every scenario and Ouisync has distinct threat scenarios, which are described below.

### Actors and tokens

Ouisync recognizes three types of non-adversary actors based on whether they posses one of the
following three cryptographic tokens or the BitTorrent infohash:

* Write token - The user of this replica may decrypt the repository content as well as make modifications to it.
* Read token - The user can decrypt the content.
* Blind token - The user is able to exchange the encrypted repository’s content with replicas having that same repository.
* BitTorrent DHT info hash - The user is able to locate other replicas of this repository on the BitTorrent DHT.

For information about how these tokens are implemented and how the privileges are enforced see the
section [Access secrets and share tokens](#access-secrets-and-share-tokens).

Actors may de-escalate token privileges in the direction Write token -> Read
token -> Blind token -> DHT infohash.  Escalation in the other direction is
cryptographically not possible.

Each type of token can be further categorized as public or private depending on whether it's shared inside a private group of users or made 
public (intentionally or by accident). To maintain the intergrity of Ouisync tokens, particularly the read and write tokens, it is important 
to share them securely off-channel (e.g. via secure messengers).

### Adversary types

Adversaries may be categorized into the following non-mutually-exclusive categories:

* Network level adversary
* User level adversary
* Adversary with physical access to the device

We assume that the network-level adversary is capable of performing network
packet sniffing, and/or route the traffic to perform man-in-the-middle (MitM)
attacks. Such adversary may be as simple as the admin of the WiFi router
currently in use, or a state actor performing these actions on the global
internet.

The user-level adversary is set to be the one who may (or may not) possess one of the three Ouisync
tokens and may (or may not) be using the same WiFi router. This adversary does not have a physical
access to the device.

The adversary with physical access to the device is one who got hold of the device. For example by borrowing it, theft or device seizure. 
Here we can distinguish between those that have the phone locked (e.g. someone who found the phone) or unlocked (e.g. someone who can force 
the user to unlock it).

Ouisync gives strong protection to the user data in a repository. An adversary
without the read or write token can not guess what the content of the repository is other than by
looking at the repository size.  This means the file contents, sizes, permissions, file and
directory counts as well as the directory structure is not accessible without the read or write
token. This is achieved by encrypting the file and directory contents and splitting them into blocks
of data equal in size (sharding).

Additionally, assuming the blind token is kept private, Ouisync also guarantees that a network-level and
user-level adversary is not able to download the encrypted repository data and metadata (blocks and
the index). This is done to protect from attacks where an adversay preemptively downloads found (locally
or on the BitTorrent DHT) repositories in the hope of retrieving the  read or write token later on, or in the case that a cryptographic 
exploit is found.

Finally, for as long as the repository is locked on the user's device, Ouisync also
cryptographically hides from an adversary having physical access to the device whether the device
owner has a read or write access to the repository. See the [Plausible deniability
section](#plausible-deniability) for details.

Ouisync currently does _not_ attempt to hide the IP addresses of peers using Ouisync alone or sharing particular repositories. This is an 
aspect Ouisync might attempt to tackle in the future, but right now the only protection from user-level and network-level adversaries 
collecting user's IP address is to turn off BitTorrent DHT and/or local discoveries.


### Confidentiality and Anonymity 

**Confidentiality**

> "information is not made available nor disclosed to unauthorized entities."

**Anonymity**

> "property that guarantees a user’s identity will not be disclosed without consent."


#### Ouisync usage detection

In order to find replicas, Ouisync emits messages that a user-level and
network-level adversary may detect to determine which IP addresses on the local
network or the internet (respectively) are using Ouisync.

These messages include:

* BitTorrent DHT protocol messages
* Local multicast packets
* Peer Exchange Protocol (PEX)
* NAT type detection
* Connection to eQualitie's caching servers

#### Repository enumeration

Ouisync will only reveal intent to sync a repository to other replicas if they also possess the blind token to that same repository.

A user-level adversary without any token is able to count the number of repositories on a replica by establishing a connection and 
performing the initial handshake.

A network-level adversary (with or without tokens) may enumerate DHT info hashes sent from a selected IP address.

An adversary with physical access to the device is currently able to retrieve blind tokens from repositories even without unlocking said 
device.

Overcoming these issues with anonymity remain part of our [future work](#future-work).

#### Peer Enumeration

A network-level adversary is able to read the plain text DHT messages to learn about DHT infohashes that a replica on a particular IP is 
announcing to.

A user-level adversary with a token or the DHT infohash is able to "visit" the BitTorrent DHT swarm corresponding to the token and enumerate 
IP addresses involved in syncing that repository.

A user-level adversary without any token is able to run a BitTorrent DHT crawler to find every existing swarm in it. These swarms can then 
be filtered to only include those that have peers communicating with the Ouisync protocol.

By enumerating peers in these swarms, the adversary is capable of _estimating_ social graphs (if two different IP addresses are announced in a swarm with otherwise low number of peers, then the probability of  users behind those IP addresses knowing each other increases).

### Integrity and Authenticity

**Integrity:**

> "data is protected from unauthorized changes to ensure that it is reliable and correct."

**Authenticity:**

> "means that one can establish that the content originated from a trusted entity."

Every Ouisync snapshot consists of a merkle-tree-like data structure which has hashes of the
encrypted blocks as its leaves. The root hash of the merkle tree is signed by the private key
contained in the write token. By verifying this signature, replicas quickly determine
the integrity and authenticity of every snapshot.

Each snapshot is also associated with a monotonically increasing version
vector. In this way old snapshots are not re-introduced to replicas which already
have newer version of the repository.

See the [Storage](#storage) section for details.

### Plausible deniability

One of the important security features of Ouisync is **Plausible Deniability** - this means that the
user can plausibly deny that they possess the read or write access secret to a locked repository
stored on their device. Ouisync does it by making repositories with blind access indistinguishable
from those with read/write access for as long as they are locked.

Note however, that the GUI application allows the user to “Remember Password” (where the secret is 32 
Bytes generated either randomly or through a password hashing function). The plausible deniability argument does not hold when this option 
is enabled.

This is achieved by always filling in fields with random data which would normally not be present or
would be empty. This includes the encrypted read and write keys in the metadata table (see section
[Metadata and local passwords](#metadata-and-local-passwords) for more information).

Since storing other people's data without being able to access it is one of the 
three Ouisync features, a user can always claim that any locked repository on their device is just a blind repository 
they store on someone else's behalf.

Ouisync also supports another type of plausible deniability: user can plausibly deny they have
a write access to a repository while admitting they only have read access. To do this they must
use two separate local passwords: one for reading and one for writing. The metadata contains two
separate entries, one to store the read key encrypted with the local read password and one to store
the write key encrypted with the local write password. If the user truly has only read access the
write secret entry is filled with random junk instead, analogously to the blind token usecase. Note that even
if only one local password is used, the metadata still contains two separate entries, one for the
write key and one for the read key. They are just encrypted using the same password.

## Technical description

### Disclaimer

This documentation is not a formal specification. Currently the only truly authoritative source of
truth is the source code ([app]((https://github.com/equalitie/ouisync-app), [library](https://github.com/equalitie/ouisync)).

### Access secrets and share tokens

Access to a repository is controlled with **Access secrets** which are encoded into **Share
tokens**. These sections describes both in detail.

The access secrets are computed as follows:

1. First a 256-bit number (called "Seed") is generated with a cryptographically secure RNG.
2. This number is then interpreted as a ED25519 signing private key which is called the **Write key**.
3. The corresponding public key is called the **Repository ID**.
4. Finally the salted hash of the seed, interpreted as a symmetric secret key is called the **Read key**.

The read key is used to encrypt and decrypt the content of the repository. The write key is used to
sign the snapshots and the repository id to validate them.

To share a repository in write mode, only the write key is needed because the other two secrets can
be derived from it. To share in read mode both the repository id and the read key are needed, as
one can't be derived from the other.

The repository id, despite being a _public_ key, should still be treated as secret for private
repositories as its possession allows one to find the BitTorrent DHT info-hash and thus the IP
addresses of peers syncing this reposotiry and sync with them in the blind mode.

The **share token** is an encoding of the access secrets designed for convenient sharing with peers.
It's formatted as an regular HTTPS URL. It looks like this:

    https://ouisync.net/r#{SECRETS}?name={NAME}

The `https://ouisync.net/r#` prefix allows the share token to work as regular links. For example,
when one sends such token to a peer via a secure messaging app, the app typically renders it as a
hyperlink that can be clicked. If Ouisync is installed on the recipient's device, it typically
registers itself as a handler for such links so clicking it automatically opens Ouisync and offers
to import the repository. If Ouisync is not installed, it opens the `ouisync.net` page in the web
browser. That page contains (among other things) links to download Ouisync for various platforms.

Note that the `{SECRETS}` part is separated from the rest of the URL with the `#` character which
means it's an "anchor" and as such is not sent to the `ouisync.net` server and thus opening it in a
browser doesn't compromise the secrets. Unfortunately URL anchors are still inserted into browser's
history where an adversary may find them. Addressing this concern is further elaborated in the
[Future work](#future-work) section.

The `{SECRETS}` part is the access secrets, Base64-encoded (blind token contains only the repository
ID, read token contains both the repository id and the read key and write token contains only the
write key). It's also prefixed with a variable-length encoded version number to support future
enhancements.

The final `?` and everything after it is optional and allows to associate additional information
with the token. Currently the only supported information field is the suggested repository name
which is a human-readable name of the repository used for information purposes only. In the future
more fields might be supported (for example the IP address of the sender to bypass peer discovery).

### Storage

A repository is conceptually a folder (with files and sub-folders) but that's not how it's actually
stored on disk. To provide the necessary security guarantees, data is stored in a custom format in
a SQLite database (in the future other db backends might be supported as well). This section
describes this format in detail.

#### Blobs

Files and directories are both represented as **Blobs** (a finite sequence of arbitrary bytes).
Directories are just special files with a custom format (it contains a list of the directory
entries), and so in the following section this document will use the term blob to refer to both
files and directories.

Each blob is associated with a unique, 256-bit long number called the **Blob ID**. The root
directory's blob ID is hardcoded constant of all-zeroes. All the other blob IDs are randomly
generated (with a [CSPRNG](link-csprng)). To find a blob id for a particular blob at a given path,
one needs to open it's parent directory and lookup the blob ID by the blob name. To open the parent,
it's parent might need to be opened first, and so on all the way to the root (the root's blob id is
constant so doesn't need to be looked up).

#### Blocks

A blob consist of fixed-sized chunks called **Blocks**. The size of a block is **32768 bytes**
(32KiB). The zero-based index of a block within its blob is called the **Block number** (that is,
the first block's number is 0, the second is 1, etc...).

This section describes how a blob is encoded into blocks.

First, the plaintext content of the blob is split into chunks. The first chunk has `32760` bytes
(the block size minus 8 bytes), the second and all the following chunks all have `32768` bytes
(the full block size). If the last chunk doesn't fill the whole block size the remaining bytes are
filled with zeroes.

The first chunk is then prefixed with the length of the whole blob as a **64-bit unsigned,
little-endian integer** (8 bytes) so it too becomes 32KiB long.

Then for each chunk, it's block is calculated as follows:

1. **Block nonce** is a 256-bit long number computed as `HASH(read_key || blob_id || block_number || plaintext_content)`.
2. **Block secret key** is a symmetric encryption key computed as `HASH(read_key || block_nonce)`.
3. **Block ciphertext** is computed by encrypting the chunk plaintext with the block secret key
using an unauthenticated symmetric cipher (currently `ChaCha20`).
4. **Block ID** is computed as `HASH(block_ciphertext || block_nonce)`.
5. Finally **Block** is the
(Block Id, Block nonce, Block ciphertext) triple.

Rationale:

The nonce is calculated the way it is so that two blocks that have different plaintext or are part of
different blobs or at different positions in the same blob have always *different nonces* so nonce
reuse is avoided. At the same time, two blocks that have the same plaintext, same blob and are at
the same position, but each in a different *branch*, have the *same nonce* and thus the same block
ID which is important for the correctness and performance of the *merge algorithm*.

The above implies that **deduplication** is not supported, that is, two identical plaintext chunks
are encoded into two distinct blocks with distinct block ids. The reason for this is a worry that
deduplication could reveal some small amount of information about the repository which could be
exploited by an adversary. It's not clear yet how serious this risk actually is but out of
abundance of caution deduplication is currently disabled. If a more thorough security analysis
proves that the risk is negligible then it can be enabled in the future. To do that, only the nonce
formula needs to be changed to `HASH(read_key || plaintext_context)`.

The reason the encryption doesn't use authentication ([AEAD](link-wiki-aead)) is that authentication
is implemented at the level of snapshots using the write key. This is detailed in the following
section.

#### Index

In order to find which blocks belong to a particular blob (and in which order) a separate structure
called **Index** is used. This can be conceptually thought of as a lookup table whose keys are
`(blob_id, block_numer)` pairs and values are block ids. The way the index is actually stored in the
database is quite different from this conceptual view and this section describes it in detail.

Index consist of **Branches** and those in turn consist of **Snapshots**:

A branch is a subset of the index associated with a particular *writer replica* which is identified
by an unique identifier called the **Writer ID**. A branch contains all the content created by that
replica. It exists as a means to differentiate the branches.

A snapshot is a subset of a branch and represents a single edit of a repository (e.g., creating a
file, writing to a file, moving/renaming a file, etc...). Snapshot is represented as a [Merkle
tree](link-wiki-merkle-tree) with one **Root node**, `N` layers of **Inner nodes** (currently `N=3`)
and one layer of **Leaf nodes**. Each root and `N-1` inner node layers contains up to 256 children.
The last (`N`th) inner node layer can have any number of children but having it bigger than 256 by
orders of magnitude might degrade performance.

Assuming max 256 nodes in the `N`th inner node layer yields the maximum number of blocks being
`256^4`, or `128` terabytes.

The root node contains the writer ID, [version vector](link-wiki-version-vector), hash of its
children and a cryptographic signature.

The version vector consist of `(writer_id, version: unsigned integer)`  pairs and serves to causally
order the snapshots (forms a [happened-before
relation](https://en.wikipedia.org/wiki/Happened-before)).  If a snapshot `A` has version vector
that is *happened-before* that of a snapshot `B`, then it's said that `A` is outdated relative to
`B`. Outdated snapshots are removed in a processes called **Pruning**. In some cases, outdated
snapshots may be preserved (temporarily or permanently), for example to support *backups*. If two
(or more) snapshots have version vectors that are *concurrent* then the snapshots are preserved
until the conflicts are resolved by a writer replica (which often happens automatically in a
processes called **Merging** but in some cases requires manual intervention). Ouisync upholds an
invariant that **no two snapshots from the same branch may be concurrent** and thus the concurrent
snapshots are always, by definition, from different branches (any invariant violation is detected
and the offending snapshot rejected).

The signature is computed by signing the `(writer_id, hash, version_vector)` tuple with the *write
key*. Because the snapshot is a Merkle tree, this signature provides authenticity and integrity of
the whole snapshot including all the blocks it's referencing. Thus an adversary who doesn't posses
the write key can't tamper with the data in any way as such tampering would be immediately detected
by any replica with at least blind access (because the blind token is the public key corresponding
to the write key and so can verify the signature) and such tampered snapshot would be rejected.

The inner node contains the hash of its children. The inner nodes belonging to the same parent node
are sorted into numbered **buckets**. Each inner node can have at most 256 siblings so the bucket is
represented as a single byte.

The leaf node contains a **locator hash** and a block id. The **Locator hash** is computed as `HASH
(blob_id || block_number || read_key)`. The unhashed `(blob_id, block_number)` pair is called
the **Locator**. The reason the locator is keyed-hashed is to prevent leaking information about the
number and sizes of the blobs in the repository to anyone who doesn't posses the read key.

To find a block ID corresponding to a given *locator* in a given *branch*, the following algorithm
is used:

1.	The latest root node in the branch is loaded (the latest meaning its version vector is happened-after that of all the other root nodes
in the same branch. Because of the invariant, there is always exactly one such node).
2.	The inner node that is a child of the root node from the previous step and whose bucket is the first byte of the locator hash is loaded.
3.	The inner node that is a child of the inner node from the previous step and whose bucket is the second byte of the locator hash is
loaded.
4.	The inner node that is a child of the inner node from the previous step and whose bucket is the third byte of the locator hash is loaded
5.	The leaf node that is a child of the inner node from the previous step and whose locator hash matches the locator that's being looked up
is loaded and its block id is returned.

Currently Ouisync uses 3 inner layers but the above algorithm could be extended to any number of
inner layers up to 32 which is the byte length of the locator hash.

A snapshot typically shares most of its nodes with other snapshots (even across branches). Thus any
inner or leaf node has always only one parent within a single branch but can have multiple parents
across branches. Thus while the snapshot is a tree, the index is a [DAG](link-dag) . This allows
very efficient transfer across replicas as only the nodes and blocks that are different need to be
transferred. This means that snapshot can be conceptually thought of as diff.

#### Metadata and local passwords

The access secrets / tokens are the canonical way to access a repository. They are however not very
convenient for a day-to-day use as most people are unable/unwilling to remember random 256-bit
numbers. For this reason Ouisync supports alternative way of accessing the repository - the **Local
password**. This password is used to access a repository only on a single device. Local password
should never be revealed to others, not even to people to share the repository with.

This section describes the way local password works:

The Ouisync repository database contains a table for storing **Metadata**. They are arbitrary
key/value pairs of byte strings. The values can optionaly be encrypted. When the user opts to use
local password, a **Local secret key** is first generated using a [KDF](link-kdf) from the user
provided local password and a randomly generated salt (the salt is then stored in the metadata).
Then the actual access secret (read key / write key) is encrypted using this local secret and the
resulting ciphertext stored in the metadata. To open the repository, the user needs to provide the
local password which is then hashed using the KDF to obtain the local secret key which is then used
to decrypt the access secret which is then used to actually open the repository.

### Network protocol

⚠️ **TODO** ⚠️: Talk about peer discovery and establishing connections (NAT traversal, ...)

#### Connections

In ouisync two replicas can be connected to each other using more than one physical connection. The
reason for this is to support different transports  (e.g. TCP, QUIC, Bluetooth, ...) or even
multiple instances of a single transport (e.g. two replicas that simultaneously connect to each
other via TCP end up with two connections). All physical connections to a single peer are
multiplexed into a single logical connection to avoid duplicating messages. This logical connection
is then further demultiplexed into multiple virtual connections, where each such connection is used
only for communication pertaining to a single repository.

#### Initial handshake

After a physical connection is established, the parties perform a simple initial handshake protocol:
They start by exchanging the protocol version and then exchanging their runtime ids. A **runtime
id** is the public key of an ephemeral public-private keypair which is generated at ouisync startup
and stored only in memory and so is not preserved across restarts. It is this runtime id that
enables the multiplexing of multiple physical connection into one logical connection (all physical
connections with the same runtime ID belong to the same logical connection) and also to prevent
self-connections.

Each initial handshake is signed by the ephemeral private key and then verified by the receiving
peer to ensure an attacker is not impersonating another replica in one of the logical connections.

Note this handshake is currently not encrypted as the data being exchanged is not considered
sensitive. We might consider switching to use a simple unauthenticated encryption for it to at
least prevent eavesdropping.

#### Encryption in transit

After the plain text connection between two replicas is established, each replica attempts to create
a virtual connection per repository. Each of these connections is encrypted with a symmetric key
computed by hashing the repository id together with the runtime ids of the two replicas.

The goal is that only the replicas that posses the repository id (the blind token) are allowed to
share it because only they can decrypt such messages.

Ouisync uses transport encryption and authentication in order to prevent eavesdropping and
man-in-the-middle attacks.  This means that unless the man-in-the-middle attacker has the blind
token and an eaverdroper, they will not be able to decrypt the synchronization messages. These
include:

* The writer IDs
* What version of the repository each replica has
* Which encrypted blocks belong to those versions
* Which blocks are present/missing on those replicas
* The encrypted blocks themselves

This encryption also adds _forward secrecy_, meaning that a network-level adversary with or without
the blind token, passively observing and logging the network traffic will not be able to decrypt the
messsage exchange even if the blind token is compromised in the future.

The particular encryption used is the **Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s** protocol from
the [Noise Protocol Framework](https://noiseprotocol.org/noise.html).

### Cryptographic primitives and implementations

Ouisync uses the following cryptographic primitives:

- **random number generator** : [`ThreadRng`](https://docs.rs/rand/latest/rand/rngs/struct.ThreadRng.html) from [rand](https://crates.io/crates/rand).
- **hash**: BLAKE3 from [blake3](https://crates.io/crates/blake3)
- **symmetric encryption**: ChaCha20Poly1305 for Encryption with AEAD from [chacha20poly1305](https://crates.io/crates/chacha20poly1305) and ChaCha20 for Encryption without AEAD from [chacha20](https://crates.io/crates/chacha20)
- **digital signature**: EdDSA with SHA2-512 and Curve25519 from [ed25519-dalek](https://crates.io/crates/ed25519-dalek)
- **encryption protocol**: Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s from the Noise Protocol Framework from [noise-protocol](https://crates.io/crates/noise-protocol) with [noise-rust-crypto](https://crates.io/crates/noise-rust-crypto)
- **Password hash / KDF**: [**Argon2**](https://en.wikipedia.org/wiki/Argon2)

## Future work

⚠️ **TODO** ⚠️: Elaborate on bullet points

* Ouisync tokens are stored by the browser history - allow for creation of non-URL type tokens and
  add javascript to https://ouisync.net/r to remove anchors from the history.
* List approaches how Ouisync can improve anonymity and confidentiality (Tor, multi-hop
  syncing,...) and their pros and cons.

[link-csprng](https://en.wikipedia.org/wiki/Cryptographically_secure_pseudorandom_number_generator)
[link-kdf](https://en.wikipedia.org/wiki/Key_derivation_function)
[link-dag](https://en.wikipedia.org/wiki/Directed_acyclic_graph)
[link-ouisync-app](https://github.com/equalitie/ouisync-app)
[link-ouisync-library](https://github.com/equalitie/ouisync)
[link-wiki-aead](https://en.wikipedia.org/wiki/Authenticated_encryption#Authenticated_encryption_with_associated_data_(AEAD))
[link-wiki-merkle-tree](https://en.wikipedia.org/wiki/Merkle_tree)
[link-wiki-version-vector](https://en.wikipedia.org/wiki/Version_vector)
