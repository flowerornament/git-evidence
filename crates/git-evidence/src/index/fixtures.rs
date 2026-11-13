//! Fixtures the crate's unit tests share: an index caught up over a scratch
//! repository, a run written as a producer writes it, and what another build
//! of this tool leaves in the same index file.

use super::Index;
use crate::store::Store;
use crate::testing::Scratch;
use rusqlite::Connection;

/// The trunk a scratch repository lands on.
pub(crate) const TRUNK: &str = "refs/heads/master";

/// The index over `scratch`, caught up.
pub(crate) fn index(scratch: &Scratch) -> Index {
    let store = Store::open(scratch.path().join(".git"));
    let mut index = Index::open(store, TRUNK).expect("open");
    index.catch_up().expect("catch up");
    index
}

/// What a binary of this views version built from other source leaves,
/// written on a second connection: `measures` without `metric`, a
/// violation its fold reported, and its own source.
pub(crate) fn another_build_prepares(path: &std::path::Path) {
    Connection::open(path)
        .expect("second connection")
        .execute_batch(
            "DROP VIEW measures;
             CREATE VIEW measures AS SELECT run, key FROM all_measures;
             INSERT INTO violations (kind, tree, problem, cause) VALUES ('k', 't', 'p', 'fold');
             UPDATE meta SET value = 'sha256:another-build' WHERE key = 'source';",
        )
        .expect("another build's preparation");
}

/// One complete perf run with no results on the newest landing, written
/// as a producer writes it: no catch-up of the index.
pub(crate) fn record_a_run(scratch: &Scratch) {
    let open = crate::format::event::Open::new(
        scratch.landing(0),
        crate::format::id::ProducerName::parse("tests").expect("producer"),
        "tests/1",
        &crate::format::id::Machine::parse("here").expect("machine"),
    );
    scratch
        .evidence()
        .run(&crate::format::id::Kind::parse("perf").expect("kind"), open)
        .expect("open")
        .finish()
        .end(crate::format::event::End::Complete)
        .expect("close");
}
