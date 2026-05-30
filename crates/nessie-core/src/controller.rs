//! Standard NES controller emulation.
//!
//! The original NES exposes two controller ports at CPU addresses `$4016`
//! (player 1) and `$4017` (player 2). The host strobes the controller by
//! writing the low bit of `$4016`: writing `1` latches the current state of
//! the eight buttons into an internal shift register, writing `0` ends the
//! strobe so subsequent reads shift the latched bits out one at a time.
//!
//! Each read returns the next button bit in the canonical NES order:
//!
//! 1. A
//! 2. B
//! 3. Select
//! 4. Start
//! 5. Up
//! 6. Down
//! 7. Left
//! 8. Right
//!
//! After all 8 bits have been shifted out, real hardware returns `1` on
//! subsequent reads (the "open bus" quirk). This implementation reproduces
//! that behaviour so games that rely on it (e.g. *Paperboy*) work correctly.

/// Identifies which controller port a [`Controller`] is wired to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Player {
    /// Port 1 (`$4016`).
    One,
    /// Port 2 (`$4017`).
    Two,
}

impl Player {
    /// Convenience: index into a `[Controller; 2]` array.
    #[inline]
    pub const fn index(self) -> usize {
        match self {
            Player::One => 0,
            Player::Two => 1,
        }
    }
}

/// One of the eight NES controller buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    /// Primary action button.
    A,
    /// Secondary action button.
    B,
    /// `Select` button (typically used for menus / mode switching).
    Select,
    /// `Start` button (typically used to pause / confirm).
    Start,
    /// D-pad up.
    Up,
    /// D-pad down.
    Down,
    /// D-pad left.
    Left,
    /// D-pad right.
    Right,
}

impl Button {
    /// Bit position of this button inside the controller's 8-bit state and
    /// shift register. Matches the order the real hardware shifts out
    /// (A first, Right last).
    #[inline]
    pub const fn bit(self) -> u8 {
        match self {
            Button::A => 0,
            Button::B => 1,
            Button::Select => 2,
            Button::Start => 3,
            Button::Up => 4,
            Button::Down => 5,
            Button::Left => 6,
            Button::Right => 7,
        }
    }
}

/// A standard NES controller's state.
///
/// The host calls [`Controller::set_button`] from input events; the CPU bus
/// drives [`Controller::write_strobe`] and [`Controller::read`] in response
/// to writes / reads of `$4016` / `$4017`.
#[derive(Debug, Clone, Copy)]
pub struct Controller {
    /// Current button state, bit `i` set iff [`Button::bit`] `i` is pressed.
    state: u8,
    /// Shift register driven by reads while strobe is `false`.
    shift: u8,
    /// `true` while the host is holding the strobe line high; reads return
    /// the live A-button state and the shift register is continually
    /// reloaded.
    strobe: bool,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    /// Build a freshly powered-on controller with all buttons released.
    pub const fn new() -> Self {
        Self {
            state: 0,
            shift: 0,
            strobe: false,
        }
    }

    /// Update one button's pressed/released state. When the strobe line is
    /// held high the shift register is reloaded so the latch always reflects
    /// the current state, matching real hardware.
    pub fn set_button(&mut self, button: Button, pressed: bool) {
        let mask = 1u8 << button.bit();
        if pressed {
            self.state |= mask;
        } else {
            self.state &= !mask;
        }
        if self.strobe {
            self.shift = self.state;
        }
    }

    /// Service a write to `$4016`. Only bit 0 is meaningful: `1` latches the
    /// state and engages "live" reads of the A button; `0` ends the strobe
    /// so subsequent reads shift out the latched bits.
    pub fn write_strobe(&mut self, value: u8) {
        let new_strobe = (value & 1) != 0;
        self.strobe = new_strobe;
        if new_strobe {
            self.shift = self.state;
        }
    }

    /// Service a read of `$4016` / `$4017`.
    ///
    /// Returns the next button bit (0 or 1) in the canonical order. While
    /// the strobe is high, returns the current state of the A button. After
    /// all 8 bits have been shifted out, returns `1` (the "open bus" quirk
    /// that real games depend on).
    pub fn read(&mut self) -> u8 {
        if self.strobe {
            self.state & 1
        } else {
            let bit = self.shift & 1;
            // Shift register fills with `1` from the high end after 8 reads
            // so subsequent reads return 1 forever, matching real hardware.
            self.shift = (self.shift >> 1) | 0x80;
            bit
        }
    }

    /// Snapshot of the current pressed-button bitmap (test/diagnostic use).
    #[inline]
    pub fn state(&self) -> u8 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press_all(c: &mut Controller) {
        for b in [
            Button::A,
            Button::B,
            Button::Select,
            Button::Start,
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
        ] {
            c.set_button(b, true);
        }
    }

    #[test]
    fn strobe_latches_state_and_reads_in_button_order() {
        let mut c = Controller::new();
        c.set_button(Button::A, true);
        c.set_button(Button::Start, true);
        c.set_button(Button::Right, true);

        // Strobe high → low to latch.
        c.write_strobe(1);
        c.write_strobe(0);

        let bits: Vec<u8> = (0..8).map(|_| c.read()).collect();
        // A=1, B=0, Select=0, Start=1, Up=0, Down=0, Left=0, Right=1
        assert_eq!(bits, vec![1, 0, 0, 1, 0, 0, 0, 1]);
    }

    #[test]
    fn open_bus_returns_one_after_eight_reads() {
        let mut c = Controller::new();
        press_all(&mut c);
        c.write_strobe(1);
        c.write_strobe(0);
        for _ in 0..8 {
            // All buttons pressed → every shifted bit is 1.
            assert_eq!(c.read(), 1);
        }
        // Subsequent reads keep returning 1 thanks to the open-bus emulation.
        for _ in 0..16 {
            assert_eq!(c.read(), 1);
        }
    }

    #[test]
    fn strobe_high_returns_live_a_button_state() {
        let mut c = Controller::new();
        c.write_strobe(1);
        c.set_button(Button::A, true);
        assert_eq!(c.read(), 1);
        c.set_button(Button::A, false);
        assert_eq!(c.read(), 0);
    }

    #[test]
    fn strobe_reload_after_state_change() {
        let mut c = Controller::new();
        c.set_button(Button::A, true);
        c.write_strobe(1);
        c.set_button(Button::B, true); // Re-latched because strobe is high.
        c.write_strobe(0);
        // Now A=1, B=1.
        assert_eq!(c.read(), 1);
        assert_eq!(c.read(), 1);
    }
}
