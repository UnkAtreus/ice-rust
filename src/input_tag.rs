//! Tags for synthetic events and the cursor restore decision (G4).
//!
//! Every posted event carries a unique tag in field 42
//! (`kCGEventSourceUserData`). A listen-only tap reads the field back; the
//! cursor is restored once the tap has seen the last event of a batch, or
//! when the fallback timer fires, whichever comes first, and only once.

/// High 16 bits of every tag, so ours are told apart from other apps' user
/// data. Positive as an `i64`.
pub const TAG_MARKER: u64 = 0x1CE5;

/// Hands out tags: marker, 16 bits of a per-process seed, and a 32-bit
/// counter that skips zero. Untagged events carry zero, so no tag is zero.
#[derive(Clone, Debug)]
pub struct TagSource {
    prefix: u64,
    counter: u32,
}

impl TagSource {
    pub fn new(seed: u32) -> Self {
        Self {
            prefix: (TAG_MARKER << 48) | (u64::from(seed & 0xFFFF) << 32),
            counter: 0,
        }
    }

    pub fn next_tag(&mut self) -> i64 {
        self.counter = self.counter.wrapping_add(1);
        if self.counter == 0 {
            self.counter = 1;
        }
        // The marker keeps bit 63 clear, so the value is positive.
        (self.prefix | u64::from(self.counter)) as i64
    }

    /// `count` consecutive tags.
    pub fn batch(&mut self, count: usize) -> Vec<i64> {
        (0..count).map(|_| self.next_tag()).collect()
    }
}

/// Whether a field-42 value was issued by a `TagSource`.
pub fn is_ours(tag: i64) -> bool {
    tag > 0 && (tag as u64) >> 48 == TAG_MARKER
}

/// Decides the one cursor restore of a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreGate {
    last: i64,
    done: bool,
}

impl RestoreGate {
    /// `last` is the tag of the last event in the batch.
    pub fn new(last: i64) -> Self {
        Self { last, done: false }
    }

    pub fn last_tag(&self) -> i64 {
        self.last
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The tap saw an event with `tag`. True exactly once: when it is the
    /// batch's last tag and the restore has not happened.
    pub fn observe(&mut self, tag: i64) -> bool {
        if self.done || tag != self.last {
            return false;
        }
        self.done = true;
        true
    }

    /// The fallback timer fired. True if the restore has not happened.
    pub fn expire(&mut self) -> bool {
        if self.done {
            return false;
        }
        self.done = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tags_are_unique_nonzero_and_marked() {
        let mut source = TagSource::new(23108);
        let tags = source.batch(10_000);
        let unique: HashSet<i64> = tags.iter().copied().collect();
        assert_eq!(unique.len(), tags.len());
        assert!(tags.iter().all(|&t| t > 0 && is_ours(t)));
    }

    #[test]
    fn batch_is_consecutive() {
        let mut source = TagSource::new(1);
        let tags = source.batch(3);
        assert_eq!(tags[1] - tags[0], 1);
        assert_eq!(tags[2] - tags[1], 1);
        assert_eq!(source.next_tag() - tags[2], 1);
    }

    #[test]
    fn seeds_give_disjoint_tags() {
        let a = TagSource::new(100).next_tag();
        let b = TagSource::new(101).next_tag();
        assert_ne!(a, b);
        // Only the low 16 bits of the seed are used.
        assert_eq!(
            TagSource::new(0x1_0005).next_tag(),
            TagSource::new(5).next_tag()
        );
    }

    #[test]
    fn counter_wrap_skips_zero() {
        let mut source = TagSource::new(0);
        source.counter = u32::MAX;
        let tag = source.next_tag();
        assert_eq!(tag as u64 & 0xFFFF_FFFF, 1);
        assert!(is_ours(tag));
    }

    #[test]
    fn foreign_values_are_not_ours() {
        assert!(!is_ours(0));
        assert!(!is_ours(-1));
        assert!(!is_ours(42));
        assert!(!is_ours(0x1CE4 << 48));
        assert!(is_ours(0x1CE5 << 48 | 9));
    }

    #[test]
    fn gate_fires_on_last_tag_only() {
        let mut source = TagSource::new(7);
        let tags = source.batch(3);
        let mut gate = RestoreGate::new(tags[2]);
        assert!(!gate.observe(tags[0]));
        assert!(!gate.observe(tags[1]));
        assert!(!gate.observe(0));
        assert!(!gate.is_done());
        assert!(gate.observe(tags[2]));
        assert!(gate.is_done());
        // Seen twice (the click posts its up event twice): no second restore.
        assert!(!gate.observe(tags[2]));
    }

    #[test]
    fn fallback_fires_once_and_blocks_the_tap() {
        let mut gate = RestoreGate::new(5);
        assert!(gate.expire());
        assert!(!gate.expire());
        assert!(!gate.observe(5));
    }

    #[test]
    fn tap_first_makes_fallback_a_no_op() {
        let mut gate = RestoreGate::new(5);
        assert!(gate.observe(5));
        assert!(!gate.expire());
        assert_eq!(gate.last_tag(), 5);
    }
}
