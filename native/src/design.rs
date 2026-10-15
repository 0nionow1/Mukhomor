use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Design {
    pub theme: crate::appearance::Theme,
    pub interface_size: crate::appearance::InterfaceSize,
    pub effects: bool,
    pub animation: bool,
    pub animation_speed: u32,
    pub rounding: bool,
}
impl Default for Design {
    fn default() -> Self {
        Self {
            theme: Default::default(),
            interface_size: Default::default(),
            effects: true,
            animation: true,
            animation_speed: 100,
            rounding: true,
        }
    }
}
impl Design {
    pub fn read(value: &Value) -> Self {
        Self {
            theme: crate::appearance::Theme::read(value["theme"].as_str()),
            interface_size: crate::appearance::InterfaceSize::read(
                value["interface_size"].as_str(),
            ),
            effects: value["effects"].as_bool().unwrap_or(true),
            animation: value["animation"].as_bool().unwrap_or(true),
            animation_speed: value["animation_speed"]
                .as_u64()
                .unwrap_or(100)
                .clamp(25, 200) as u32,
            rounding: value["rounding"].as_bool().unwrap_or(true),
        }
    }
    pub fn value(self) -> Value {
        json!({"theme":self.theme.id(),"interface_size":self.interface_size.id(),"effects":self.effects,"animation":self.animation,"animation_speed":self.animation_speed,"rounding":self.rounding})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_migrates_old_preferences_and_roundtrips_independently() {
        let old = Design::read(
            &json!({"effects":false,"animation":false,"animation_speed":75,"rounding":false}),
        );
        assert_eq!(old.theme, crate::appearance::Theme::Neon);
        assert_eq!(old.interface_size, crate::appearance::InterfaceSize::Medium);
        assert!(!old.effects);
        for theme in crate::appearance::Theme::ALL {
            for interface_size in crate::appearance::InterfaceSize::ALL {
                let design = Design {
                    theme,
                    interface_size,
                    ..old
                };
                assert_eq!(Design::read(&design.value()), design);
            }
        }
        let malformed = Design::read(&json!({"theme":[],"interface_size":999}));
        assert_eq!(malformed, Design::default());
    }
    #[test]
    fn animation_speed_migrates_and_bounds_saved_input() {
        assert_eq!(Design::read(&json!({})).animation_speed, 100);
        for (input, expected) in [
            (json!(25), 25),
            (json!(175), 175),
            (json!(0), 25),
            (json!(u64::MAX), 200),
            (json!(-1), 100),
            (json!("fast"), 100),
        ] {
            let design = Design::read(&json!({"animation_speed":input}));
            assert_eq!(design.animation_speed, expected);
            assert_eq!(Design::read(&design.value()), design);
        }
    }
}
