
# About

This document is a description of how OuiSync stores encrypted blocks on disk
such that it can synchronize these blocks with other peers while also maintain
divergent branches.

# CryFS block store

CryFS stores encrypted data in files of size (by default) 32768. The file names
are random numbers (truly random, _not_ hashes of the data or something like
that) represented in the code by the BlockId structure. 

This means there can be a two files whith the same BlockId but different content.

# Problem #1

Two diverging branches likely share a lot of the same blocks. Thus we don't want to
store each such shared block separately.

# Problem #2

Adding/removing a new branch must be quick. I.e. it should require less than
O(# of blocks) file operations (assuming only a small number of blocks differ
in the two branches).

# Problem #3

It should be quick to find differences between two different commits. I.e.
assuming the number of distinct blocks is small, it should take less than
O(# of blocks) to find them.

# Solution draft/example

Say we have two branches B1 and B2. The head of B1 is C12 and the head of B2 is
C21 like so:

```
             +-> block ID
             |    +-> block data (encrypted)
             |    |
B1/C12 = { (123, aaa), (124, bbb), (234, ccc), (235, ddd), (236, fff) }
B2/C21 = { (123, aaa), (124, bbb), (234, ccc), (235, eee),            }
                                                           |--------|
                                                           Missing in B2
                                               |--------|
                                               Different in B1 and B2
```

```
divergent-heads/
+ B1/
  + version-vector
+ B2/
  + version-vector
blocks/
+ hashes = { (B1, hash), (B2, hash) }
+ 1/
  + hashes = { (B1, hash), (B2, hash) }
  + 2/
    + 3/
      + data = aaa
    + 4/
      + data = bbb
+ 2/
  + hashes = { (B1, hash), (B2, hash) }
  + 3/
    # A directory with a single subdirectory doesn't need the "hashes" file
    + 4/
      + data = ccc
    + 5
      + conflict = { (B1,hash(ddd)), (B2,hash(eee)) }
      + data.hash(ddd) = ddd
      + data.hash(eee) = eee
    + 6/
      + conflict = { (B1,hash(fff)), (B2,) }
      + data.hash(fff) = fff
```

```
read(block_id, branch):
    dir = block_id_to_path(block_id)
    data = fs::read(dir/data)
    # The optimistic case is the fastest
    if data: return data
    conflicts = fs::read(dir/conflict)
    return fs::read("dir/data." + conflicts.hash_of(branch))
```

# Journaling

Say we have a simplified problem where we have two files: `data` and `version`.
The `data` file contains arbitrary data and `version` contains an unsigned integer
that is incremented each time data is changed. The problem at hand is to devise
a scheme such that the `vesion` of the data is always correct.

Naive implementation where this doesn't hold:

1. start with `data` containing "a" and `version` containing "1"
2. open the `data` file and modify the content to "b"
3. open `version` file to modify the version
4. program crashes
5. program restarts and `version` file contains "1" but `data` contains "b"

Correct implementation:

1. start with `data` containing "a" and `version` containing "1"
2. move `data` to `data.1` (extension indicating the current content of `version`)
3. create new `data` file
4. content(`data`) = "b"
5. open(`version`)
6. content(`version`) = "2"
7. delete `data.1`

## Crash recovery

If the program crashes anywhere between steps 2 and 7, the program rolls
back content(`version`) to 1 (based on the extension of the data.1 file)
and moves `data.1` back to `data`.
