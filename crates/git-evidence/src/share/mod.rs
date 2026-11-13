//! Sharing evidence between stores: [`remote`] fetches and pushes each kind's
//! notes ref, and [`audit`] reads what a push would publish first.

pub(crate) mod audit;
pub(crate) mod remote;
