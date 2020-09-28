
**Definition _ObjectId_**: _ObjectId_ is an array of 16 random bytes.

**Definition _Version_**: _Version_ is a 64 unsigned integer

**Definition _VersionVector_**: _VersionVector_ is a map from UUID to Version.
Additionally, the absence of a particular (UUID, Version) in a version vector
is equivalent to having an entry for that UUID in the map with Version equal to
zero.

**Definition _EncryptedData_**: _EncryptedData_ is a map from ObjectId to encrypted
32KB data blocks.

**Definition _Commit_**: _Commit_ is the pair (VersionVector, EncryptedData).

**Notation**: We denote by C_{VersionVector} the Commit (VersionVector, EncryptedData).
(edited)

**Definition _Branch_**: A _Branch_ is a set of commits where for each two distinct
commits c_i and c_j either c_i < c_j or c_j < c_i.

**Definition _Branch HEAD_**: A _Branch HEAD_ is a commit H in the branch for which
each other commit c_i of that branch holds that c_i < H.

**Definition _Branch ID_**: An array of 16 random bytes that locally represent
a branch.
