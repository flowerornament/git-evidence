# git-process

A hardened `git` subprocess for Rust programs that read or write a Git
repository by running the `git` command.

- **One invocation prefix.** Every command runs with no automatic gc, objects
  and refs flushed to disk before it returns, a fixed locale and no prompts.
  `isolated` removes the inherited variables that would redirect Git to
  another repository or configuration.
- **Typed object ids.** `CommitId`, `TreeId` and `ObjectId` are validated
  where they are parsed.
- **Starts that survive a busy machine.** `start()` retries a start that the
  operating system refused for lack of resources. It reports each failed
  attempt to an optional observer, installed once with
  `observe_start_failures`. Missing or forbidden executables and oversized
  argument vectors refuse at once. An executed command is never repeated;
  collecting its output is a separate failure from starting it.
- **One time form.** `Timestamp` is a UTC time to the millisecond, written
  `YYYY-MM-DDTHH:MM:SS.mmmZ` and parsed once where it is read.

It knows nothing about what a caller stores in the repository.

```rust,no_run
use git_process::Git;

let git = Git::at(".git");
let head = git.resolve("HEAD")?; // Some(object id), or None if HEAD is unborn
let tree = git.tree_of(&head.expect("a commit"))?;
# Ok::<(), git_process::GitError>(())
```

The `testing` feature adds `testing::repo`: a throwaway repository with one
commit, for tests of Git readers.

`git-evidence` is built on it and released with it at the same version.

## License

MIT or Apache-2.0, at your option.
