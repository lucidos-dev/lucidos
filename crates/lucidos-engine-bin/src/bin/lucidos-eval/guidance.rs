use crate::arm::SweepPins;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn guidance_for_this_build() -> Fallible<String> {
    Ok(rendered_at(SweepPins::from_env()?))
}

pub fn rendered_at(sweep: SweepPins) -> String {
    lucidos_engine::engine::rendered_context_mode_prompt(
        sweep.expire_after_rounds,
        sweep.sweep_every_rounds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_in_this_checkout_renders_the_guidance() {
        let guidance = guidance_for_this_build().expect("no schedule pin is set in a test run");
        assert!(
            guidance.contains("YOUR WORKING UNDERSTANDING"),
            "the guidance read back is not the mode's: {}",
            guidance.chars().take(120).collect::<String>()
        );
    }

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
