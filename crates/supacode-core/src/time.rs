//! Dates relatives en français : « à l'instant », « il y a 3 min », « hier »…

use std::time::{Duration, SystemTime};

pub fn relative(when: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(when).unwrap_or(Duration::ZERO).as_secs();
    match secs {
        0..60 => "à l'instant".into(),
        60..3_600 => format!("il y a {} min", secs / 60),
        3_600..86_400 => format!("il y a {} h", secs / 3_600),
        86_400..172_800 => "hier".into(),
        172_800..2_592_000 => format!("il y a {} j", secs / 86_400),
        _ => format!("il y a {} mois", secs / 2_592_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libelles() {
        let now = SystemTime::now();
        let ago = |s| relative(now - Duration::from_secs(s), now);
        assert_eq!(ago(5), "à l'instant");
        assert_eq!(ago(180), "il y a 3 min");
        assert_eq!(ago(7_200), "il y a 2 h");
        assert_eq!(ago(90_000), "hier");
        assert_eq!(ago(3 * 86_400), "il y a 3 j");
    }
}
