/// Turns periodic samples of a bouncy, noisy push-button-like input into
/// one event per press: both the press and the release must stay stable for
/// `stable_samples` consecutive samples, so short spikes (e.g. induced into
/// long cables by the motors switching) and contact bounce are ignored.
pub struct Debouncer {
    stable_samples: u8,
    /// true = released, waiting for a press; false = pressed, waiting for release.
    armed: bool,
    stable: u8,
}

impl Debouncer {
    pub const fn new(stable_samples: u8) -> Self {
        Debouncer {
            stable_samples,
            armed: true,
            stable: 0,
        }
    }

    /// Feeds one sample (`true` = pressed); returns `true` exactly once per
    /// accepted press.
    pub fn sample(&mut self, pressed: bool) -> bool {
        // While armed we're looking for a press, otherwise for a release.
        if pressed != self.armed {
            self.stable = 0;
            return false;
        }
        self.stable += 1;
        if self.stable < self.stable_samples {
            return false;
        }
        self.stable = 0;
        let fired = self.armed;
        self.armed = !self.armed;
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Indices of the samples at which a press was reported.
    fn presses(samples: &str) -> Vec<usize> {
        let mut debouncer = Debouncer::new(5);
        samples
            .chars()
            .filter(|c| !c.is_whitespace())
            .enumerate()
            .filter(|&(_, c)| debouncer.sample(c == '#'))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn clean_press_fires_once_after_five_samples() {
        assert_eq!(presses("..... ########## ....."), [9]);
    }

    #[test]
    fn holding_does_not_repeat() {
        assert_eq!(presses(&"#".repeat(1000)), [4]);
    }

    #[test]
    fn spikes_shorter_than_five_samples_are_ignored() {
        assert_eq!(presses("..#.. .##.. ..###. ####. ....."), []);
    }

    #[test]
    fn bounce_on_press_and_release_counts_once() {
        assert_eq!(
            presses("#.#.##.##### ###### .#..#.#....... ##### ....."),
            [11, 36]
        );
    }

    #[test]
    fn a_new_press_needs_a_stable_release_first() {
        // Released for only 4 samples in between: still the same press.
        assert_eq!(presses("##### .... ##### ..... #####"), [4, 23]);
    }
}
