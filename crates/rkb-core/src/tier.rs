//! Lesson tiers, derived from the use records over every machine.
//!
//! The store grows without a limit; only a small, proven part reaches an agent session. A lesson is
//! `candidate` until a `worked` record, then `proven`; the `archived` and `superseded` statuses are
//! tombstones. The tier is computed on read from the counts, never stored in a lesson file, so it
//! cannot drift from the records it follows.

use serde::Serialize;

use crate::lesson::Status;
use crate::usage::Counts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Candidate,
    Proven,
    Archive,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Candidate => "candidate",
            Tier::Proven => "proven",
            Tier::Archive => "archive",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "candidate" => Some(Tier::Candidate),
            "proven" => Some(Tier::Proven),
            "archive" => Some(Tier::Archive),
            _ => None,
        }
    }
}

/// The tier of a lesson with `status` and its use `counts` (`None` when it has no record).
pub fn of(status: Status, counts: Option<&Counts>) -> Tier {
    if !status.is_current() {
        return Tier::Archive;
    }
    match counts.map_or(0, |c| c.worked) {
        0 => Tier::Candidate,
        _ => Tier::Proven,
    }
}

/// Why a lesson moves to `archive` on its counts, or `None` when it stays.
///
/// A lesson with no harmful record is never archived by this rule, so one bad report cannot remove a
/// lesson that worked. Days unused alone never demote: a lesson for a rare error is seldom needed and
/// still valuable.
pub fn demote(counts: &Counts) -> Option<&'static str> {
    let bad = counts.failed + counts.irrelevant;
    if counts.harmful() == 0.0 {
        return None;
    }
    // ExpeL starts each count at 2: a lesson with no `worked` needs two bad reports, so one bad
    // report cannot remove a candidate that was never used in earnest.
    if counts.worked == 0 && bad < 2 {
        return None;
    }
    if counts.harmful() > counts.helpful() {
        return Some("more harmful than helpful");
    }
    if counts.worked == 0 && bad >= 2 {
        return Some("reported irrelevant or failed twice with no worked report");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(worked: u32, inferred: u32, failed: u32, irrelevant: u32) -> Counts {
        Counts { worked, inferred, failed, irrelevant, ..Default::default() }
    }

    #[test]
    fn a_lesson_with_no_record_is_a_candidate() {
        assert_eq!(of(Status::Active, None), Tier::Candidate);
        assert_eq!(of(Status::Active, Some(&Counts::default())), Tier::Candidate);
        assert_eq!(of(Status::Stale, Some(&counts(0, 3, 0, 0))), Tier::Candidate, "inferred alone does not promote");
    }

    #[test]
    fn the_first_worked_makes_it_proven() {
        assert_eq!(of(Status::Active, Some(&counts(1, 0, 0, 0))), Tier::Proven);
        assert_eq!(of(Status::Stale, Some(&counts(4, 2, 3, 1))), Tier::Proven);
    }

    #[test]
    fn a_tombstone_is_archive_whatever_its_counts() {
        assert_eq!(of(Status::Archived, Some(&counts(9, 0, 0, 0))), Tier::Archive);
        assert_eq!(of(Status::Superseded, None), Tier::Archive);
    }

    #[test]
    fn demote_needs_a_bad_signal() {
        assert!(demote(&Counts::default()).is_none(), "no record is never archived");
        assert!(demote(&counts(1, 0, 0, 1)).is_none(), "one bad report cannot remove a lesson that worked");
        assert_eq!(demote(&counts(1, 0, 0, 2)), Some("more harmful than helpful"));
        assert_eq!(demote(&counts(0, 0, 2, 0)), Some("more harmful than helpful"));
        assert_eq!(demote(&counts(0, 0, 1, 1)), Some("more harmful than helpful"));
        // A lesson with no `worked` and only one bad report stays: ExpeL starts each count at 2.
        assert!(demote(&counts(0, 1, 0, 1)).is_none());
        assert!(demote(&counts(0, 3, 1, 0)).is_none());
        // The second clause catches a lesson whose inferences balance the bad reports.
        assert_eq!(demote(&counts(0, 4, 2, 0)), Some("reported irrelevant or failed twice with no worked report"));
        // A lesson with no harmful record is never archived, whatever its age or inferences.
        assert!(demote(&counts(0, 9, 0, 0)).is_none());
    }
}
