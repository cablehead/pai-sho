//! Whether to dial a peer, and which connection to keep when both ends have
//! one. Pure.

use iroh::EndpointId;

/// Whether we call a peer or wait for it to call us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reach {
    /// We took up its invitation, or it has connected to us. Either way we
    /// dial it (ADR 0008).
    reached: bool,
    /// It said goodbye: it is going to sleep and will call us when it wakes
    /// (ADR 0009).
    asleep: bool,
}

impl Reach {
    /// A peer whose invitation we took up, or one that has connected.
    pub fn dialed() -> Self {
        Self {
            reached: true,
            asleep: false,
        }
    }

    /// A peer we invited that has not called yet.
    pub fn invited() -> Self {
        Self {
            reached: false,
            asleep: false,
        }
    }

    /// It connected to us. It has proven its key, and it is awake.
    pub fn connected_in(&mut self) {
        self.reached = true;
        self.asleep = false;
    }

    /// It said goodbye. Wait for it to call.
    pub fn said_goodbye(&mut self) {
        self.asleep = true;
    }

    pub fn asleep(&self) -> bool {
        self.asleep
    }

    /// Whether to dial it now. `we_sleep` is true once we have said goodbye
    /// ourselves: we are about to be suspended, and we call when we wake.
    pub fn dials(&self, we_sleep: bool) -> bool {
        self.reached && !self.asleep && !we_sleep
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Keep {
    /// Keep what we have
    Held,
    /// Take the new one
    Arriving,
}

/// Which of two connections to the same peer we keep.
///
/// Both ends dial after first contact, so we can hold one we opened and see
/// one they opened at the same time. Both ends keep the one the lower key
/// opened. A closed slot always takes the new one. If the same side dialed
/// both, the new one wins: they just redialed.
pub fn resolve(
    ours: &EndpointId,
    theirs: &EndpointId,
    held_live: bool,
    held_dialed_by_us: bool,
    arriving_dialed_by_us: bool,
) -> Keep {
    if !held_live {
        return Keep::Arriving;
    }
    if held_dialed_by_us == arriving_dialed_by_us {
        return Keep::Arriving;
    }
    let we_are_lower = ours.as_bytes() < theirs.as_bytes();
    let lower_key_opened_held = we_are_lower == held_dialed_by_us;
    if lower_key_opened_held {
        Keep::Held
    } else {
        Keep::Arriving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn key(n: u8) -> EndpointId {
        SecretKey::from_bytes(&[n; 32]).public()
    }

    /// Two keys, lower first.
    fn pair() -> (EndpointId, EndpointId) {
        let (a, b) = (key(1), key(2));
        if a.as_bytes() < b.as_bytes() {
            (a, b)
        } else {
            (b, a)
        }
    }

    #[test]
    fn an_invited_peer_is_waited_for_until_it_calls() {
        let mut reach = Reach::invited();
        assert!(!reach.dials(false));
        reach.connected_in();
        assert!(reach.dials(false));
    }

    #[test]
    fn a_peer_that_said_goodbye_is_not_dialed() {
        let mut reach = Reach::dialed();
        assert!(reach.dials(false));
        reach.said_goodbye();
        assert!(reach.asleep());
        assert!(!reach.dials(false));
    }

    #[test]
    fn a_sleeping_peer_is_awake_once_it_calls() {
        let mut reach = Reach::dialed();
        reach.said_goodbye();
        reach.connected_in();
        assert!(!reach.asleep());
        assert!(reach.dials(false));
    }

    /// An invited peer that says goodbye before we ever dialed it still
    /// waits, and dials once it calls.
    #[test]
    fn goodbye_from_an_invited_peer_still_waits_for_its_call() {
        let mut reach = Reach::invited();
        reach.said_goodbye();
        assert!(!reach.dials(false));
        reach.connected_in();
        assert!(reach.dials(false));
    }

    #[test]
    fn we_dial_no_one_once_we_have_said_goodbye() {
        assert!(!Reach::dialed().dials(true));
        assert!(!Reach::invited().dials(true));
    }

    #[test]
    fn a_reconnect_from_the_dialer_is_not_a_race() {
        let (low, high) = pair();
        // We hold what they dialed. A second dial from them replaces it.
        assert_eq!(resolve(&low, &high, true, false, false), Keep::Arriving);
        assert_eq!(resolve(&high, &low, true, false, false), Keep::Arriving);
    }

    #[test]
    fn a_closed_slot_takes_arriving() {
        let (low, high) = pair();
        assert_eq!(resolve(&low, &high, false, true, false), Keep::Arriving);
        assert_eq!(resolve(&high, &low, false, true, true), Keep::Arriving);
    }

    #[test]
    fn the_lower_key_keeps_its_own_dial() {
        let (low, high) = pair();
        assert_eq!(resolve(&low, &high, true, true, false), Keep::Held);
    }

    #[test]
    fn the_higher_key_takes_theirs() {
        let (low, high) = pair();
        assert_eq!(resolve(&high, &low, true, true, false), Keep::Arriving);
    }

    #[test]
    fn both_ends_keep_the_lower_keys_dial() {
        let (low, high) = pair();
        // Each holds its own dial and sees the other's arrive.
        assert_eq!(resolve(&low, &high, true, true, false), Keep::Held);
        assert_eq!(resolve(&high, &low, true, true, false), Keep::Arriving);
    }

    #[test]
    fn inbound_then_our_dial_still_keeps_the_lower_keys_dial() {
        let (low, high) = pair();
        // Inbound landed first: we hold theirs, then our own dial returns.
        assert_eq!(resolve(&low, &high, true, false, true), Keep::Arriving);
        assert_eq!(resolve(&high, &low, true, false, true), Keep::Held);
    }
}
