//! Which scene comes next: a shuffled deck or the list in order, with a history for `prev`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Scenes remembered for `prev`.
const HISTORY: usize = 100;

/// xorshift64: hold choices, pools and the shuffle only need to look random.
pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Rng {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Rng::from_seed(nanos)
    }

    pub fn from_seed(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Scene order. Shuffled: a deck dealt again once every scene has shown, never repeating the scene on screen.
/// In order: the list as given, wrapping around. Either way `prev` walks back through what was shown.
pub struct Rotation {
    scenes: Vec<String>,
    shuffle: bool,
    deck: Vec<String>,
    history: Vec<String>,
    position: usize,
}

impl Rotation {
    pub fn new(scenes: Vec<String>, first: Option<String>, shuffle: bool, rng: &mut Rng) -> Rotation {
        let mut rotation = Rotation {
            scenes,
            shuffle,
            deck: Vec::new(),
            history: Vec::new(),
            position: 0,
        };
        let first = match first {
            Some(name) => {
                // The starting scene has shown: deal the rest of the first deck without it.
                if shuffle {
                    rotation.deal_deck(rng);
                    rotation.deck.retain(|s| *s != name);
                }
                name
            }
            None if shuffle => rotation.deal(rng),
            None => rotation.scenes[0].clone(),
        };
        rotation.history.push(first);
        rotation
    }

    pub fn current(&self) -> &str {
        &self.history[self.position]
    }

    /// Forward through the history, or a new scene.
    pub fn next(&mut self, rng: &mut Rng) -> String {
        if self.position + 1 < self.history.len() {
            self.position += 1;
        } else {
            let scene = if self.shuffle { self.deal(rng) } else { self.following() };
            self.history.push(scene);
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
            self.position = self.history.len() - 1;
        }
        self.current().to_string()
    }

    /// Back through the history; None at its start.
    pub fn prev(&mut self) -> Option<String> {
        self.position = self.position.checked_sub(1)?;
        Some(self.current().to_string())
    }

    /// The scene after the current one in the list, wrapping around.
    fn following(&self) -> String {
        let at = self
            .scenes
            .iter()
            .position(|s| s == self.current())
            .map_or(0, |i| i + 1);
        self.scenes[at % self.scenes.len()].clone()
    }

    fn deal_deck(&mut self, rng: &mut Rng) {
        self.deck = self.scenes.clone();
        for i in (1..self.deck.len()).rev() {
            self.deck.swap(i, rng.below(i + 1));
        }
    }

    fn deal(&mut self, rng: &mut Rng) -> String {
        if self.deck.is_empty() {
            self.deal_deck(rng);
        }
        // Never the scene already showing, when there is another.
        if self.deck.len() > 1 && self.history.get(self.position) == self.deck.last() {
            let last = self.deck.len() - 1;
            self.deck.swap(0, last);
        }
        self.deck.pop().unwrap_or_else(|| self.scenes[0].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenes() -> Vec<String> {
        ["a", "b", "c", "d"].map(String::from).to_vec()
    }

    #[test]
    fn shuffle_shows_every_scene_once_per_deck() {
        let mut rng = Rng::from_seed(42);
        let mut rotation = Rotation::new(scenes(), None, true, &mut rng);
        let mut deck = vec![rotation.current().to_string()];
        for _ in 1..4 {
            deck.push(rotation.next(&mut rng));
        }
        deck.sort();
        assert_eq!(deck, scenes());
    }

    #[test]
    fn shuffle_never_repeats_the_scene_on_screen() {
        let mut rng = Rng::from_seed(7);
        let mut rotation = Rotation::new(scenes(), Some("b".into()), true, &mut rng);
        let mut last = rotation.current().to_string();
        for _ in 0..200 {
            let next = rotation.next(&mut rng);
            assert_ne!(next, last);
            last = next;
        }
    }

    #[test]
    fn in_order_wraps_and_starts_at_the_first_scene() {
        let mut rng = Rng::from_seed(1);
        let mut rotation = Rotation::new(scenes(), None, false, &mut rng);
        assert_eq!(rotation.current(), "a");
        let shown: Vec<String> = (0..5).map(|_| rotation.next(&mut rng)).collect();
        assert_eq!(shown, ["b", "c", "d", "a", "b"]);
        let mut from_c = Rotation::new(scenes(), Some("c".into()), false, &mut rng);
        assert_eq!(from_c.next(&mut rng), "d");
    }

    #[test]
    fn prev_walks_the_history_and_next_returns_along_it() {
        let mut rng = Rng::from_seed(3);
        let mut rotation = Rotation::new(scenes(), None, false, &mut rng);
        assert_eq!(rotation.prev(), None);
        rotation.next(&mut rng);
        rotation.next(&mut rng);
        assert_eq!(rotation.prev(), Some("b".into()));
        assert_eq!(rotation.prev(), Some("a".into()));
        assert_eq!(rotation.prev(), None);
        assert_eq!(rotation.next(&mut rng), "b");
        assert_eq!(rotation.next(&mut rng), "c");
        assert_eq!(rotation.next(&mut rng), "d");
    }

    #[test]
    fn history_is_capped() {
        let mut rng = Rng::from_seed(5);
        let mut rotation = Rotation::new(scenes(), None, false, &mut rng);
        for _ in 0..(HISTORY * 2) {
            rotation.next(&mut rng);
        }
        assert_eq!(rotation.history.len(), HISTORY);
        assert_eq!(rotation.position, HISTORY - 1);
    }
}
