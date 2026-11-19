//! Who was turned away at the door, and why. Pure: the shell passes the time
//! in, so the window and the bound are unit tests rather than sleeps.
//!
//! An inviter refuses a key it does not know that presents no live code. The
//! person on the other side sees nothing they can act on, and before this the
//! only trace here was a log line. `list` shows these, so an operator can tell
//! a visitor with an expired invitation from one who never arrived, and admit
//! the key directly with `invite <key>`.

use iroh::EndpointId;

use super::session::Refusal;

/// How long a refusal stays listed, in seconds.
pub const WINDOW: u64 = 60 * 60;
/// The most refusals kept. Past it, the one heard from longest ago goes.
pub const MAX: usize = 50;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refused {
    pub key: EndpointId,
    /// Why it was turned away the last time
    pub reason: Refusal,
    /// Times turned away within the window
    pub count: u32,
    /// Unix seconds of the last time
    pub last: u64,
}

#[derive(Default)]
pub struct Refusals {
    entries: Vec<Refused>,
}

impl Refusals {
    pub fn new() -> Self {
        Self::default()
    }

    /// `key` was turned away for `reason` at `now`.
    pub fn record(&mut self, key: EndpointId, reason: Refusal, now: u64) {
        self.prune(now);
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.count += 1;
            entry.reason = reason;
            entry.last = now;
            return;
        }
        self.entries.push(Refused {
            key,
            reason,
            count: 1,
            last: now,
        });
        if self.entries.len() > MAX {
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last)
                .map(|(i, _)| i);
            if let Some(i) = oldest {
                self.entries.remove(i);
            }
        }
    }

    /// `key` got in: it is a peer now, not a visitor at the door.
    pub fn clear(&mut self, key: &EndpointId) {
        self.entries.retain(|e| &e.key != key);
    }

    /// Refusals within the window at `now`, latest first.
    pub fn recent(&self, now: u64) -> Vec<Refused> {
        let mut out: Vec<Refused> = self
            .entries
            .iter()
            .filter(|e| now.saturating_sub(e.last) <= WINDOW)
            .cloned()
            .collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.last));
        out
    }

    fn prune(&mut self, now: u64) {
        self.entries
            .retain(|e| now.saturating_sub(e.last) <= WINDOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn key(n: u8) -> EndpointId {
        SecretKey::from_bytes(&[n; 32]).public()
    }

    #[test]
    fn a_second_refusal_counts_against_the_same_key() {
        let mut r = Refusals::new();
        r.record(key(1), Refusal::NotAuthorized, 100);
        r.record(key(1), Refusal::NotAuthorized, 110);
        let recent = r.recent(110);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].count, 2);
        assert_eq!(recent[0].last, 110);
    }

    #[test]
    fn the_latest_reason_is_kept() {
        let mut r = Refusals::new();
        r.record(key(1), Refusal::EnrollTimeout, 100);
        r.record(key(1), Refusal::NotAuthorized, 110);
        assert_eq!(r.recent(110)[0].reason, Refusal::NotAuthorized);
    }

    #[test]
    fn latest_first() {
        let mut r = Refusals::new();
        r.record(key(1), Refusal::NotAuthorized, 100);
        r.record(key(2), Refusal::NotAuthorized, 200);
        let keys: Vec<_> = r.recent(200).iter().map(|e| e.key).collect();
        assert_eq!(keys, vec![key(2), key(1)]);
    }

    #[test]
    fn a_refusal_older_than_the_window_is_not_listed() {
        let mut r = Refusals::new();
        r.record(key(1), Refusal::NotAuthorized, 100);
        assert_eq!(r.recent(100 + WINDOW).len(), 1);
        assert!(r.recent(100 + WINDOW + 1).is_empty());
    }

    #[test]
    fn an_old_refusal_starts_counting_again() {
        let mut r = Refusals::new();
        r.record(key(1), Refusal::NotAuthorized, 100);
        r.record(key(1), Refusal::NotAuthorized, 100 + WINDOW + 1);
        assert_eq!(r.recent(100 + WINDOW + 1)[0].count, 1);
    }

    #[test]
    fn past_the_bound_the_one_heard_from_longest_ago_goes() {
        let mut r = Refusals::new();
        for n in 0..=MAX as u8 {
            r.record(key(n + 1), Refusal::NotAuthorized, 1000 + n as u64);
        }
        let recent = r.recent(1000 + MAX as u64);
        assert_eq!(recent.len(), MAX);
        assert!(!recent.iter().any(|e| e.key == key(1)));
    }

    #[test]
    fn a_key_that_got_in_is_no_longer_listed() {
        let mut r = Refusals::new();
        r.record(key(1), Refusal::NotAuthorized, 100);
        r.clear(&key(1));
        assert!(r.recent(100).is_empty());
    }
}
