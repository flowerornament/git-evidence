//! What is written in the notes and how it is read back: the event lines
//! ([`event`]), the validated names and ids they carry ([`id`]), and the fold
//! that reads a note's lines into runs ([`fold`]).

pub(crate) mod event;
pub(crate) mod fold;
pub(crate) mod id;
