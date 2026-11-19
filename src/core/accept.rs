//! The accepting side of an invitation: when a refusal ends it, and what
//! `accept` answers. Pure.
//!
//! Taking up an invitation used to answer `OK` at once and leave the peer
//! saved, whatever the inviter made of the code. A dead code was refused on
//! every dial, so the peer flickered between online and connecting for good,
//! with no ports and no word why. Now `accept` waits for the inviter's answer,
//! and a refused code ends the attempt instead of retrying it.

use super::session::Refusal;

/// What the accepting side has heard from a peer, as `accept` watches it.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Joining {
    /// Port lists the peer has sent us. An inviter sends one the moment it
    /// admits us, even an empty one, so the first means we are in.
    pub announces: u64,
    /// The inviter refused our code, and the attempt is over
    pub ended: Option<String>,
    /// The inviter turned us away, but we keep trying: we presented no code,
    /// so it may yet invite our key
    pub turned_away: Option<String>,
}

/// What `accept` tells the person who ran it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The peer let us in. Its ports are whatever it announced.
    Joined,
    /// The peer refused the invitation. Nothing is kept.
    Refused(String),
    /// No answer in time. The peer stays saved and is dialed with backoff.
    Pending { turned_away: Option<String> },
}

/// Whether a close `reason` is the other side turning us away, rather than a
/// connection replaced, a goodbye, or a network drop.
pub fn is_refusal(reason: &[u8]) -> bool {
    [Refusal::NotAuthorized, Refusal::EnrollTimeout]
        .iter()
        .any(|r| reason == r.as_str().as_bytes())
}

/// Whether the inviter closing a connection with `reason` ends our attempt.
///
/// Only a refused code does: the code is expired or used, and presenting it
/// again cannot work. That holds for a peer that let us in before too, since
/// the inviter forgets a known peer that presents a stale code. A peer we dial
/// by key alone keeps trying, because the other side may run `invite <key>`
/// after we `accept` (ADR 0006).
pub fn refusal_ends_invitation(reason: &[u8], presented_code: bool) -> bool {
    presented_code && reason == Refusal::NotAuthorized.as_str().as_bytes()
}

/// `accept`'s answer, given what it has heard since it started (`baseline`
/// announces), or None to keep waiting. Once the wait is up, a peer we are
/// connected to has joined with what it has; one we never reached is pending.
pub fn verdict(now: &Joining, baseline: u64, timed_out: bool, connected: bool) -> Option<Verdict> {
    if let Some(reason) = &now.ended {
        return Some(Verdict::Refused(reason.clone()));
    }
    if now.announces > baseline {
        return Some(Verdict::Joined);
    }
    if !timed_out {
        return None;
    }
    if connected {
        return Some(Verdict::Joined);
    }
    Some(Verdict::Pending {
        turned_away: now.turned_away.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOT_AUTHORIZED: &[u8] = b"not authorized";

    #[test]
    fn a_refused_code_ends_the_attempt() {
        assert!(refusal_ends_invitation(NOT_AUTHORIZED, true));
    }

    #[test]
    fn a_bare_key_keeps_trying() {
        assert!(!refusal_ends_invitation(NOT_AUTHORIZED, false));
    }

    #[test]
    fn an_enroll_timeout_is_worth_another_try() {
        assert!(!refusal_ends_invitation(b"enroll timeout", true));
    }

    #[test]
    fn any_other_close_is_worth_another_try() {
        assert!(!refusal_ends_invitation(b"replaced", true));
        assert!(!refusal_ends_invitation(b"", true));
    }

    #[test]
    fn only_a_turn_away_is_a_refusal() {
        assert!(is_refusal(NOT_AUTHORIZED));
        assert!(is_refusal(b"enroll timeout"));
        assert!(!is_refusal(b"replaced"));
        assert!(!is_refusal(b"goodbye"));
        assert!(!is_refusal(b"not granted"));
    }

    #[test]
    fn an_announce_after_the_start_is_joined() {
        let now = Joining {
            announces: 3,
            ..Default::default()
        };
        assert_eq!(verdict(&now, 2, false, true), Some(Verdict::Joined));
        assert_eq!(verdict(&now, 3, false, true), None);
    }

    #[test]
    fn a_refusal_wins() {
        let now = Joining {
            announces: 1,
            ended: Some("not authorized".into()),
            turned_away: None,
        };
        assert_eq!(
            verdict(&now, 0, false, false),
            Some(Verdict::Refused("not authorized".into()))
        );
    }

    #[test]
    fn out_of_time_and_connected_is_joined_with_what_it_has() {
        assert_eq!(
            verdict(&Joining::default(), 0, true, true),
            Some(Verdict::Joined)
        );
    }

    #[test]
    fn out_of_time_and_never_reached_is_pending() {
        assert_eq!(
            verdict(&Joining::default(), 0, true, false),
            Some(Verdict::Pending { turned_away: None })
        );
    }

    #[test]
    fn pending_says_when_it_was_turned_away() {
        let now = Joining {
            turned_away: Some("not authorized".into()),
            ..Default::default()
        };
        assert_eq!(
            verdict(&now, 0, true, false),
            Some(Verdict::Pending {
                turned_away: Some("not authorized".into())
            })
        );
    }
}
