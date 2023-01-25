# Maintaining Merkle Trees at the filesystem level

by Samuel Grayson

Merkle Tree datastructures are widely used for comparing file trees, but they can be expensive to compute.
The contribution of this work will be a design, implementation, and evaluation of a system that maintains a Merkle Tree at the filesystem level.
Every file operation in the filesystem is augmented to incrementally maintain the full or partial Merkle Tree, from which computin the full Merkle Tree should be faster than doing so from scratch.
Applications that use a Merkle Tree of files (e.g. version control and file synchronization operations) can use this Merkle Tree computed by the filesystem.
This might accelerate those applications that can use a Merkle Tree.

## Background

A Merkle Tree is a tree datastructure that makes it easy to test if two trees are the same and identify the differences if not [[Wikipedia][Wikipedia Merkle tree]].
Every node `n` stores a hash `n.hash` based on its children `n.children` and the data at that node `n.data` is a hash of the concatenation of the hashes of the children and the hash of the data at that node.
In pseudo-Python: `n.hash = hash(concatenate_strings(append([child.hash | for child in sorted(n.children)], hash(n.data))))`.
If two nodes have the same hash, then barring a hash-collision, they have the same `child.hash`, so all of their descendants are the same too, with high-probability.
We can detect if two Merkle Trees are identical in $\mathcal O(1)$ by comparing the root node hash.
If not, we can find each of the $k$ differing node in a $h$-tall $\mathcal O(k h)$; Usually $h = \log(n)$, where $n$ is the number of nodes.
This even offers an opportunity for deduplicated storage: if two Merkle Tree nodes have the same hash, they only need to be stored once.

## Motivation

If Merkle Trees of a filesystem tree were easily available, it could provide two related operations:

1. Is this file-tree identical to that file-tree?
2. If not, What are the differences those file-trees?

These operations are commonly applied in file synchronization and version control.

- File synchronization is the problem of taking two near-copies and copying the differing files from the first to the second.
  This is commonly done by `rsync` [[man rsync][man rsync]].
  `rsync` has options to compare files either by modification time or by hash.
  Modification time is fast (as fast as reading file metadata), but is prone to false-positives (two subsequent `wget -O- $URL | tar xz` will have different modification time) and false-negatives.
  A more robust approach is to use file hashes, but then `rsync` has to calculate and compare these hashes on-the-fly.
  Other solutions, like DropBox use `inotify` to learn of file system changes; however, inotify doesn't know about changes that occur while the inotify daemon is not running, so every startup the system has to do a full enumeration of all files, to see if any have changed.
  If a Merkle Tree was maintained at the OS-level, it could accelerate `rsync`'s determination of which files are different.

- Version control systems like `git` already use a Merkle Tree to deduplicate and efficiently compute different revisions of the source tree.
  Since large tech companies shifted towards monorepos, their codebases hit up against the scaling limits of `git` [[Goode and Rain][Goode and Rain]].
  Even a simple `git status` on a big repository can take seconds to complete which slows down development as `git status` is almost as frequent as `ls`.
  If the OS maintained a Merkle Tree, `git status` and other operations could be faster.
  Git also has `core.fsmonitor`, which uses inotify to accelerate some operations, and suffers from the same problems as DropBox above.

- Antivirus scanners, build systems, workflow managers and webservers with `--watch` need to know if the underlying files have changed since the last time they were scanned or compiled.
  They have varying strategies; AV scanners periodically rescan, since they cannot trust that the filesystem time has not been tampered with.
  Build systems and workflow managers generally check the modification time or hash, which as with rsync, is either error-prone or expensive.
  Webservers with `--watch` generally use inotify, which means they always have to do a fresh build when they start up.

Merkle Tree algorithms give a speedup on random changes to the file tree, but the real world should benefit even more than those results would predict.
In the real world, file changes are not random; they are often localized to just a few high-churn directories.
For example, if one is developing a new controller for a web application, most of their changes will likely be in a directory dedicated to that controller.
If there are $k$ diferences in the same directory, a Merkle Tree will find them in $\mathcal O(k + h)$ rather than $\mathcal O(kh)$ when the changes were in random directories.

## Prior Work

ZFS does maintain a Merkle Tree at the filesystem level, but it uses data that is below the filesystem abstraction (e.g. block pointers) [[Stackexchange answer][stackexchange]].
This allows ZFS to implement copy-on-write, but it does not accelerate applications that sit above the filesystem abstraction, that is they only care about the _contents_ of the file, not its physical storage.
These would almost always differe on two different systems even if the files were identical.

Git also maintains a Merkle Tree in the `.git` directory.
However, unlike the filesystem, Git is not alerted when a file is modified, so it has to recompute the Merkle Tree from scratch by hashing every file.

## Design

Every time a user requests a hash of a file or directory, the filesystem should cache that hash and store one extra bit indicating that the hash is "valid".
Future requests for that hash do not need to be recomputed, if the cached valid bit is set.
If a file or directory is modified, the filesystem needs to invalidate (unset the valid bit) the cached hash of that file and all of its ancestor directories.
If the filesystem needs the hash of an invalid node, it will recompute hash and mark it as valid.

```
def modify(Node n):
    n.valid = False
    if n.parent is not None:
        modify(n.parent)

def hash(Node n):
    if not n.valid:
        n.hash = hash(
            concatenate_strings(append([child.hash | for child in sorted(n.children)], hash(n.data)))
        )
        n.valid = True
    return n.hash

def check_invariant(Node n):
    if n.valid:
        assert n.hash == hash(
            concatenate_strings(append([child.hash | for child in sorted(n.children)], hash(n.data)))
        ) # hash validity invariatn
        assert all(child.valid | for child in n.children) # recursive validity invariant
    for child in n.children:
        check_invariant(child)
```

The time cost of updating a file is proportional to the height of that file in the filesystem, which can lead to increased overheads during a write.
However, the algorithm need only climb "down" (towards the root of the tree) until it gets to an invalid directory; its ancestors that should already be invalid.
Only the first invalidation climbs down to the root; everything else stops short.

As far as I know, this has not been evaluated in prior literature, so it remains an open question if the overheads outweigh the benefits and which strategy (described below) is the most performant.

## Implementation

I will call the hypothetical implementation, MTFS (Merkle Tree File System).

This can be easily implemented natively in Linux without modifying the kernel by a Filesystem in USErspace (FUSE) [[libfuse github][libfuse github]].
There are even third-party projects for MacOS [[osxfuse github][osxfuse github]] and Windows [[winfsp github][winfsp github]] that implement FUSE.
The user would ask MTFS to mount a FUSE at a specified mount-point.
The MTFS FUSE process would proxy requests from that mount-point to a private store on top of a normal filesystem.
MTFS should expose a server where applications can ask MTFS to compute the hash of a directory.
In the private store, MTFS could use "extended file attributes" on POSIX, MacOS, or Windows [[Wikipedia][Wikipedia Extended file attributes]] to create a new file attribute storing the hash and valid bit.
Alternatively, MTFS could reserve the first K bytes of every file in the private store for metadata.
Storing the metadata with the file contents maintains the consistency guarantees of the underlying filesystem (e.g. POSIX compliance).

If FUSE is not available, MTFS could use a file alteration monitor: `inotify` on Linux [[man inotify][man inotify]], `FSEvents` on MacOS [[Apple developer documentation][Apple developer documentation]].

In order to be compatible with `git`, MTFS have the option of using SHA-1 as the hashing algorithm, but it could present the user options for other hash algorithms as well.
In particular, xxhash is a fast, although not cryptographically secure, hash [[xxhash github][xxhash github]].
`git` and `rsync` also have mechanisms to ignore certain files based on string patterns; MTFS should use those same mechanisms so that the Merkle Tree can be directly usable by `git` or `rsync`.
Applications could detect the presence of MTFS by reading a extended attribute, a "fake" file, or some other means.
Whatever the mechanism, it should also indicate the location of the MTFS server, so the application can ask for a hash.

A malware may want to hide itself in legitimate code in MTFS.
If a malware could control the MTFS private store, it might surreptitiously modify (infect) a file in the MTFS mount _without_ invalidating the hash, as the protocol intends.
This would violate the hash validity invariant.
A system administrator might attempt to clean out the infected file by redeploying the tree from a known clean version, but MTFS will not know that the infected file has changed at all, so it will not overwrite the infected file with the clean file.
This is why it is important that the MTFS private store be owned by root.

## Evaluation

###  Analytical Evaluation

The "stopping short" strategy ensures that each directory is valid or only invalidated once.
Therefore the number of invalidations is the count of invalid nodes.
The cost of invalidation gets amortized across all of the writes.

I would continue in this vein, and analytically predict the number of invalidations.

### Empirical Evaluation

I would can construct artificial datasets, such as a K-ary tree.
Changesets could be empty (no files changed), 1%, 10%, or 100% of those files.
Each of these reflects a case that could happen in the real world.

The strongest empirical support would come from selecting large GitHub repositories.
I would narrow in on repositories with an large number of files or with a deep filesystem.
Then, I would use real commits as changesets.

I would modify the file selection part of `rsync` to use Merkle Trees from MTFS, if MTFS is available.
Then I would compare the performance of MTFS `rsync` versus vanilla `rsync` between an unmodified and modified directory tree.
Likewise, I would modify `git` to use MTFS to hash files, rather than hashing files itself.
Then I would time the performance of `git status` after applying a patch.
This emulates the development process of modifying the filesystem.
This evaluation would assess whether there is enough reused data between revisions to make Merkle Trees advantageous.
I would also measure the time it takes to do the actual modifications, because this would be slowed down a bit by FUSE overhead and MTFS invalidation overhead.

I would use the Suh et al's experimental protocol for measuring system runtimes [[Suh et al.][Suh et al.]].
Then, I would use Bayesian statistics to compute the distribution of runtime performance with and without MTFS.
I expect MTFS to usually outpreform vanilla for a microbenchmark constructing a Merkle Tree in all cases except the 100%-of-files-changed case.
I expect macrobenchmarks (`rsync` and `git`) runtimes on MTFS to somewhat preform non-MTFS, since the macrobenchmarks have other operations that MTFS doesn't improve.
I expect the 'tall' filesystems with few changes will incur the greatest write overhead in MTFS; it requires invalidations all the way up the tree, and those invalidations are not amortized across many writes.
If the macrobenchmark performance is usually faster by more than 10% and 2 seconds and the write performance is not slower by 10% or 30ms, then I would conclude that MTFS could be useful in those applications and use-case sizes.

## Future work

The actual stored Merkle Tree could be a relaxation or augmentation of the file tree.
Consider the following tree

```
/
/0
/0/0
/0/1
/0/2
/0/3
/1/
/1/0
/1/1
/1/2
/1/3

```

The default file tree in JSON is

```
{
  "0": {"0": data, "1": data, "2": data, "3": data},
  "1": {"0": data, "1": data, "2": data, "3": data}
}
```

A relaxation of this is:

```
{
  "0/0": data,
  "0/1": data,
  "0/2": data,
  "0/3": data,
  "1/0": data,
  "1/1": data,
  "1/2": data,
  "1/3": data
}
```

Relaxation decreases height and increases the arity.
`/0` and `/1` do not need to be explicitly stored because their existence is implied by their children `/0/0` and `/1/0`.

Alternatively, an augmentation does the opposite:
```
{
  "0": {
    "_a": {"0": data, "1": data},
    "_b": {"2": data, "3": data},
  },

  "1": {
    "_a": {"0": data, "1": data},
    "_b": {"2": data, "3": data},
  }
}
```

Augmentation introduces nodes which are not present in the filesystem.
This increases height and decreases arity.
One needs an extra bit saying if a particular interior node is a true directory on the filesystem or an augmentation.

Decreasing the arity makes hashing the directory faster, while decreasing the height makes invalidating a node faster.
Therefore, augmentation and relaxation allow us to tradeoff these two quantities.
If we encounter an abnormally heigh tree or an abnormally high arity node, we might use relaxation and augmentation to improve performance.

## License for this file

<a rel="license" href="http://creativecommons.org/licenses/by/4.0/"><img alt="Creative Commons License" style="border-width:0" src="https://i.creativecommons.org/l/by/4.0/88x31.png" /></a><br />This work is licensed under a <a rel="license" href="http://creativecommons.org/licenses/by/4.0/">Creative Commons Attribution 4.0 International License</a>.

[Wikipedia Merkle tree]: https://en.wikipedia.org/wiki/Merkle_tree
[man rsync]: https://linux.die.net/man/1/rsync
[Goode and Rain]: https://engineering.fb.com/2014/01/07/core-data/scaling-mercurial-at-facebook/
[Stackexchange]: https://serverfault.com/a/893412
[libfuse github]: https://github.com/libfuse/libfuse
[osxfuse github]: https://github.com/osxfuse/osxfuse
[winfsp github]: https://github.com/winfsp/winfsp
[xxhash github]: https://cyan4973.github.io/xxHash/
[Wikipedia Extended file attributes]: https://en.wikipedia.org/wiki/Extended_file_attributes
[Suh et al.]: https://onlinelibrary.wiley.com/doi/full/10.1002/spe.2476
[man inotify]: https://www.man7.org/linux/man-pages/man7/inotify.7.html
[Apple developer documentation]: https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html
