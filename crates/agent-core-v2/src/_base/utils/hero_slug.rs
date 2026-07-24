//! Readable, memorable hero-name slug generation.
//!
//! Original: `_base/utils/hero-slug.ts`.

use std::collections::HashSet;

pub const HERO_NAMES: &[&str] = &[
    "iron-man",
    "spider-man",
    "captain-america",
    "thor",
    "hulk",
    "black-widow",
    "hawkeye",
    "black-panther",
    "doctor-strange",
    "scarlet-witch",
    "vision",
    "falcon",
    "war-machine",
    "ant-man",
    "wasp",
    "captain-marvel",
    "gamora",
    "star-lord",
    "groot",
    "rocket",
    "drax",
    "mantis",
    "nebula",
    "shang-chi",
    "moon-knight",
    "ms-marvel",
    "she-hulk",
    "echo",
    "wolverine",
    "cyclops",
    "storm",
    "jean-grey",
    "rogue",
    "beast",
    "nightcrawler",
    "colossus",
    "shadowcat",
    "jubilee",
    "cable",
    "deadpool",
    "bishop",
    "magik",
    "iceman",
    "archangel",
    "psylocke",
    "dazzler",
    "forge",
    "havok",
    "polaris",
    "emma-frost",
    "namor",
    "silver-surfer",
    "adam-warlock",
    "nova",
    "quasar",
    "sentry",
    "blue-marvel",
    "spectrum",
    "squirrel-girl",
    "cloak",
    "dagger",
    "punisher",
    "elektra",
    "luke-cage",
    "iron-fist",
    "jessica-jones",
    "daredevil",
    "blade",
    "ghost-rider",
    "morbius",
    "venom",
    "carnage",
    "silk",
    "spider-gwen",
    "miles-morales",
    "america-chavez",
    "kate-bishop",
    "yelena-belova",
    "white-tiger",
    "moon-girl",
    "devil-dinosaur",
    "amadeus-cho",
    "riri-williams",
    "kamala-khan",
    "sam-alexander",
    "nova-prime",
    "medusa",
    "black-bolt",
    "crystal",
    "karnak",
    "gorgon",
    "lockjaw",
    "quake",
    "mockingbird",
    "bobbi-morse",
    "maria-hill",
    "nick-fury",
    "phil-coulson",
    "winter-soldier",
    "us-agent",
    "patriot",
    "speed",
    "wiccan",
    "hulkling",
    "stature",
    "yellowjacket",
    "tigra",
    "hellcat",
    "valkyrie",
    "sif",
    "beta-ray-bill",
    "hercules",
    "wonder-man",
    "taskmaster",
    "domino",
    "cannonball",
    "sunspot",
    "wolfsbane",
    "warpath",
    "multiple-man",
    "banshee",
    "siryn",
    "monet",
    "rictor",
    "shatterstar",
    "longshot",
    "daken",
    "x-23",
    "fantomex",
    "batman",
    "superman",
    "wonder-woman",
    "flash",
    "aquaman",
    "green-lantern",
    "martian-manhunter",
    "cyborg",
    "hawkgirl",
    "green-arrow",
    "black-canary",
    "zatanna",
    "constantine",
    "shazam",
    "blue-beetle",
    "booster-gold",
    "firestorm",
    "atom",
    "hawkman",
    "plastic-man",
    "red-tornado",
    "starfire",
    "raven",
    "beast-boy",
    "robin",
    "nightwing",
    "batgirl",
    "batwoman",
    "red-hood",
    "signal",
    "orphan",
    "spoiler",
    "catwoman",
    "huntress",
    "supergirl",
    "superboy",
    "power-girl",
    "steel",
    "stargirl",
    "wildcat",
    "doctor-fate",
    "mister-terrific",
    "hourman",
    "sandman",
    "spectre",
    "phantom-stranger",
    "swamp-thing",
    "animal-man",
    "deadman",
    "vixen",
    "black-lightning",
    "static",
    "icon",
    "rocket-dc",
    "captain-atom",
    "fire",
    "ice",
    "elongated-man",
    "metamorpho",
    "black-hawk",
    "crimson-avenger",
    "doctor-mid-nite",
    "jakeem-thunder",
    "mister-miracle",
    "big-barda",
    "orion",
    "lightray",
    "forager",
    "killer-frost",
    "jessica-cruz",
    "simon-baz",
    "john-stewart",
    "guy-gardner",
    "kyle-rayner",
    "hal-jordan",
    "wally-west",
    "barry-allen",
    "jay-garrick",
    "impulse",
    "kid-flash",
    "donna-troy",
    "tempest",
    "aqualad",
    "miss-martian",
    "terra",
    "jericho",
    "ravager",
    "red-star",
    "pantha",
    "argent",
    "damage",
    "jade",
    "obsidian",
    "cyclone",
    "atom-smasher",
    "maxima",
    "starman",
    "liberty-belle",
    "dove",
    "hawk",
    "blue-devil",
    "creeper",
    "ragman",
    "thunder",
];

const MAX_ATTEMPTS: usize = 20;

// Original: pickHero(). Node's crypto.randomInt is unbiased; rejection
// sampling keeps the same property without adding a general-purpose RNG crate.
fn pick_hero() -> Result<&'static str, getrandom::Error> {
    let bound = u32::try_from(HERO_NAMES.len()).unwrap_or(u32::MAX);
    let limit = u32::MAX - (u32::MAX % bound);
    loop {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes)?;
        let value = u32::from_ne_bytes(bytes);
        if value < limit {
            return Ok(HERO_NAMES[(value % bound) as usize]);
        }
    }
}

// Original: assembleSlug().
fn assemble_slug() -> Result<String, getrandom::Error> {
    Ok(format!(
        "{}-{}-{}",
        pick_hero()?,
        pick_hero()?,
        pick_hero()?
    ))
}

// Original: generateHeroSlug(). The current source caller supplies an ASCII
// UUID as `id`, so scalar slicing preserves the source fallback suffix.
pub fn generate_hero_slug(
    id: &str,
    existing: &HashSet<String>,
) -> Result<String, getrandom::Error> {
    generate_hero_slug_with(id, existing, assemble_slug)
}

fn generate_hero_slug_with(
    id: &str,
    existing: &HashSet<String>,
    mut assemble: impl FnMut() -> Result<String, getrandom::Error>,
) -> Result<String, getrandom::Error> {
    let mut slug = String::new();
    let mut collided = true;
    for _ in 0..MAX_ATTEMPTS {
        slug = assemble()?;
        if !existing.contains(&slug) {
            collided = false;
            break;
        }
    }
    if collided {
        let suffix: String = id.chars().take(8).collect();
        slug = format!("{slug}-{suffix}");
    }
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_first_non_colliding_slug() {
        let existing = HashSet::from([String::from("thor-hulk-wasp")]);
        let mut candidates = ["thor-hulk-wasp", "spider-man-vision-groot"].into_iter();
        let slug = generate_hero_slug_with("ignored", &existing, || {
            Ok(candidates.next().unwrap().into())
        })
        .unwrap();
        assert_eq!(slug, "spider-man-vision-groot");
    }

    #[test]
    fn suffixes_the_twentieth_collision_with_the_id_prefix() {
        let existing = HashSet::from([String::from("thor-hulk-wasp")]);
        let slug =
            generate_hero_slug_with("12345678-rest", &existing, || Ok("thor-hulk-wasp".into()))
                .unwrap();
        assert_eq!(slug, "thor-hulk-wasp-12345678");
    }

    #[test]
    fn exposes_the_complete_non_empty_source_vocabulary() {
        assert_eq!(HERO_NAMES.len(), 233);
        assert!(HERO_NAMES.contains(&"iron-man"));
        assert!(HERO_NAMES.contains(&"thunder"));
    }
}
