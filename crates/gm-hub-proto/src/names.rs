//! What a character may be called (CLIENT.md 7). A name is read by other people, in the
//! chat and over a body, so it is made of what every client can draw, and no two names may
//! be told apart only by a second look: the hub keeps names unique by their *skeleton*.

/// The letters ASCII lacks that the client's font has (Gaj's alphabet).
const MARKED: &str = "čćđšžČĆĐŠŽ";

/// Words that are the game's own voice, as skeletons: no name is one of them, has one of
/// them as a word of its own, or is one with a number behind it.
const RESERVED: &[&str] = &[
    "zone",
    "system",
    "server",
    "admln",
    "admlns",
    "admlnlstrator",
    "moderator",
    "moderators",
    "mod",
    "mods",
    "gm",
    "gms",
    "gamemaster",
    "staff",
    "support",
    "offlclal",
    "hub",
    "gamenglne",
    "nobody",
];

/// A character's name as the hub takes it: trimmed, two characters or more and 24 bytes
/// at most, beginning with a letter, made of letters, digits and single spaces, hyphens
/// and apostrophes between them. The error says what is wrong, for the screen that asked.
pub fn character_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.chars().count() < 2 {
        return Err("a name is two letters or more");
    }
    if name.len() > 24 {
        return Err("a name is 24 letters at most (a marked letter counts for two)");
    }
    let letter = |c: char| c.is_ascii_alphabetic() || MARKED.contains(c);
    if !name.chars().next().is_some_and(letter) {
        return Err("a name begins with a letter");
    }
    let mut joiner = false;
    for c in name.chars() {
        let joins = matches!(c, ' ' | '-' | '\'');
        if !(letter(c) || c.is_ascii_digit() || joins) {
            return Err("a name is made of letters, digits, spaces, hyphens and apostrophes");
        }
        if joins && joiner {
            return Err("a name has no two spaces or hyphens in a row");
        }
        joiner = joins;
    }
    if joiner {
        return Err("a name ends with a letter or a digit");
    }
    // The whole name, and each word of it, without the number it may end in: `Zone`,
    // `Z0ne`, `Zone 2`, `GM Bob`, `Moderator7`.
    let own = |word: &str| {
        let word = skeleton(word.trim_end_matches(|c: char| c.is_ascii_digit()));
        RESERVED.contains(&word.as_str())
    };
    if own(name) || name.split([' ', '-', '\'']).any(own) {
        return Err("that name is the game's own");
    }
    Ok(name.to_string())
}

/// What a name looks like at a glance: small letters without their marks, the digit 1 and
/// the letter i as an l (a capital I is a small l to the eye, and capitals do not tell
/// names apart), the digit 0 as an o, and nothing between the letters. Two names with one
/// skeleton are one name to the eye.
pub fn skeleton(name: &str) -> String {
    name.chars()
        .filter_map(|c| {
            Some(match c {
                'i' | 'I' | '1' | 'l' | 'L' => 'l',
                '0' | 'O' | 'o' => 'o',
                'č' | 'ć' | 'Č' | 'Ć' => 'c',
                'š' | 'Š' => 's',
                'ž' | 'Ž' => 'z',
                'đ' | 'Đ' => 'd',
                ' ' | '-' | '\'' => return None,
                c => c.to_ascii_lowercase(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_what_everybody_can_read() {
        for good in [
            "Aldric",
            "Željko",
            "Ana-Marija",
            "O'Neil",
            "Red 5",
            "Xy",
            "ć1",
            "šđ",
            "Modesty",
            "Zoner",
            "Gmitar",
        ] {
            assert_eq!(character_name(good).as_deref(), Ok(good), "{good}");
        }
        assert_eq!(character_name("  Aldric  ").as_deref(), Ok("Aldric"));
        for bad in [
            "",
            "A",
            "5ive",
            " ",
            "-Aldric",
            "Aldric-",
            "Al  dric",
            "Al- dric",
            "Aldric!",
            "Zo\u{eb}",
            "Aldric\nZone: you win",
            "An exceedingly long name!",
            "ŽŽŽŽŽŽŽŽŽŽŽŽŽ",
            // One letter, however many bytes it is written in.
            "č",
            "Đ",
        ] {
            assert!(character_name(bad).is_err(), "{bad:?} was taken for a name");
        }
    }

    #[test]
    fn names_alike_at_a_glance_have_one_skeleton() {
        // A capital I for a small l, a zero for an o, a mark more or less, a space.
        for (a, b) in [
            ("Aldric", "AIdric"),
            ("Aldric", "A1dric"),
            ("Bob", "B0b"),
            ("Čedo", "Cedo"),
            ("Ćiro", "Čiro"),
            ("De Vil", "Devil"),
            ("ALDRIC", "aldric"),
        ] {
            assert_eq!(skeleton(a), skeleton(b), "{a} and {b}");
        }
        // The price: an i and an l are one letter here, capital or small.
        assert_eq!(skeleton("Mila"), skeleton("Mlla"));
        assert_ne!(skeleton("Mila"), skeleton("Mira"));
        // The game's own voice cannot be put on.
        for staff in [
            "Zone",
            "ZONE",
            "Z0ne",
            "Admin",
            "AdmIn",
            "System",
            "Moderator",
            "G-M",
            "Gamengine",
            // As a word of its own, and with a number behind it.
            "Zone 2",
            "Zone2",
            "Z0ne 7",
            "GM Bob",
            "Bob GM",
            "Moderator7",
            "Staff",
            "Official-Zone",
        ] {
            assert_eq!(
                character_name(staff),
                Err("that name is the game's own"),
                "{staff}"
            );
        }
        // Every reserved word is stored as its own skeleton.
        for word in RESERVED {
            assert_eq!(&skeleton(word), word);
        }
    }
}
