
# About

This document is a description of how OuiSync stores encrypted blocks on disk
such that it can synchronize these blocks with other peers while also maintain
divergent branches.

# CryFS block store

CryFS stores encrypted data in files of size (by default) 32768. The file names
are random numbers (truly random, _not_ hashes of the data or something like
that) represented in the code by the BlockId structure. 

This means there can be a two files whith the same BlockId but different content.

# First draft

* writer-replica: any node that can store encrypted data as well as write to it
* blind-replica: any node that can store encrypted data but can't decrypt it
  and thus it also can't write into it

Operations (Ops) that should be fast:
  1. Given BlockId, retrieve data for local block
  2. Get list of divergent clocks

```
data/
+ vector-clock/
  * <writer-replica-uuid>          # contains a single number
+ blocks/
  + hash                           # = hash(sum_i(child[i]/hash))
  * <block_id_part_1>/
    + hash                         # = hash(sum_j(child[j]/hash))
    * <block_id_part_2>/
      + hash                       # = hash(sum_k(child[k]/hash))
      ...
        * <block_id_part_N>/
          + hash                   # = hash(data + block_id)
          + data
+ divergent-clocks/                # Having this list satisfies Ops #2
  * <vector-clock-hash>            # Contains the actual vector clock
+ conflicts/
  * <block_id>/
    * <conflicted-data-hash>/
      + data
      + vector-clocks/
        * <vector-clock-hash>
```

Note: If two divergent branches contain the same data for a `block_id`
we do not store the data twice.

