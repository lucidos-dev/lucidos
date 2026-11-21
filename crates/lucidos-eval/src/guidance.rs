//! Where the note-writing guidance comes from, and what `guidance_hash` covers.
//!
//! ADR 0087 stamps `guidance_hash` on every result row, so a run with revised
//! guidance is distinguishable from one with an edited probe. It hashed a
//! fixture copy of the text, then a string literal read out of the checkout.
//!
//! **Neither is enough once the prompt quotes a preference.** Two arms swept at
//! different expiry or sweep values render different text from the same
//! literal, so a source hash would call them identical. The one instrument that
//! distinguishes guidance versions would go blind during the very sweep it was
//! built for.
//!
//! So the hash covers the RENDERED prompt, at the schedule this run pins. The
//! harness links the engine, so it renders the same function the engine will.

use crate::arm::SweepPins;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The guidance the engine in THIS build will send, at this run's schedule.
///
/// Rendering cannot fail, but resolving the schedule can. A pin the engine
/// would reject renders guidance no arm will ever be given, so the hash would
/// label text nothing was told.
pub fn guidance_for_this_build() -> Fallible<String> {
    Ok(rendered_at(SweepPins::from_env()?))
}

/// The mode's whole prompt section, as a model reading it would see it.
pub fn rendered_at(sweep: SweepPins) -> String {
    lucidos_engine::engine::rendered_context_mode_prompt(
        sweep.expire_after_rounds,
        sweep.sweep_every_rounds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state of this repository right now, asserted rather than assumed.
    ///
    /// A prompt that lost its guidance would still hash to something, and that
    /// something would label nothing.
    #[test]
    fn the_engine_in_this_checkout_renders_the_guidance() {
        let guidance = guidance_for_this_build().expect("no schedule pin is set in a test run");
        assert!(
            guidance.contains("YOUR WORKING UNDERSTANDING"),
            "the guidance read back is not the mode's: {}",
            guidance.chars().take(120).collect::<String>()
        );
    }

    /// Invariant 43. Two arms at different values hash differently, or a sweep
    /// leaves rows nothing can tell apart.
    #[test]
    fn two_schedules_render_different_guidance() {
        let five_ten = rendered_at(SweepPins {
            expire_after_rounds: 5,
            sweep_every_rounds: 10,
        });
        let four_ten = rendered_at(SweepPins {
            expire_after_rounds: 4,
            sweep_every_rounds: 10,
        });
        let five_eight = rendered_at(SweepPins {
            expire_after_rounds: 5,
            sweep_every_rounds: 8,
        });
        assert_ne!(five_ten, four_ten);
        assert_ne!(five_ten, five_eight);
    }
}
