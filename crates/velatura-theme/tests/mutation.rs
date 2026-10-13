//! A poor man's fuzzer: tens of thousands of damaged copies of a real theme
//! file go through the parser and, when they load, through every renderer.
//! Nothing may panic, and whatever loads must render to well-formed output.
//! Deterministic, so a failure can be reproduced from its seed.

use velatura_theme::{
    Color, Resolved, bundled, ghostty, gtk, kitty, nvim, parse_theme, starship, tmux,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const SNIPPETS: [&str; 14] = [
    "\"",
    "'",
    "\n",
    "#",
    "@",
    "[",
    "]",
    "=",
    "\\",
    "\u{202e}",
    "\0",
    "{",
    "999999999999999999999",
    "[variants.default.ui]\n",
];

fn mutate(src: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = src.to_vec();
    for _ in 0..=rng.below(2) {
        let at = rng.below(out.len());
        match rng.below(6) {
            0 => out[at] = rng.next() as u8,
            1 => {
                out.remove(at);
            }
            2 => {
                let s = SNIPPETS[rng.below(SNIPPETS.len())].as_bytes();
                out.splice(at..at, s.iter().copied());
            }
            3 => {
                // Drop a whole line.
                let end = out[at..]
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(out.len(), |i| at + i);
                out.drain(at..end);
            }
            4 => {
                // Repeat a slice somewhere else.
                let len = rng.below(60).min(out.len() - at);
                let piece = out[at..at + len].to_vec();
                let to = rng.below(out.len());
                out.splice(to..to, piece);
            }
            _ => out.truncate(at),
        }
        if out.is_empty() {
            out.push(b'\n');
        }
    }
    out
}

#[test]
fn damaged_theme_files_never_panic_and_never_produce_bad_output() {
    let mut loaded = 0;
    for (n, (_, source)) in bundled::SOURCES.iter().enumerate() {
        for seed in 1..=15_000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ n as u64);
            let bytes = mutate(source.as_bytes(), &mut rng);
            let text = String::from_utf8_lossy(&bytes);
            let Ok(ok) = parse_theme(&text) else {
                continue;
            };
            loaded += 1;
            for variant in ok.theme.variants.keys() {
                let r = Resolved::new(&ok.theme, Some(variant)).unwrap();
                for line in kitty::render(&r).lines().skip(2) {
                    let (_, value) = line.split_once(' ').expect("seed reproduces");
                    assert!(Color::parse(value).is_ok(), "seed {seed}: {line:?}");
                }
                for line in ghostty::render(&r).lines().skip(2) {
                    let value = line.rsplit('=').next().unwrap().trim();
                    assert!(Color::parse(value).is_ok(), "seed {seed}: {line:?}");
                }
                for line in tmux::render(&r).lines().skip(2) {
                    let ok = line.strip_prefix("set -g ").is_some_and(|rest| {
                        rest.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b" -=#,\"".contains(&b))
                    });
                    assert!(
                        ok && line.matches('"').count() == 2,
                        "seed {seed}: {line:?}"
                    );
                }
                let lines = starship::render(&r);
                for line in lines.lines().skip(1) {
                    let value = line.rsplit_once(" = ").expect("seed reproduces").1;
                    assert!(
                        Color::parse(value.trim_matches('"')).is_ok(),
                        "seed {seed}: {line:?}"
                    );
                }
                assert!(lines.parse::<toml::Table>().is_ok(), "seed {seed}");
                // In the Lua files a theme can put nothing but colours:
                // every quoted string is a fixed word or a colour.
                for text in [nvim::render(&r), nvim::render_palette(&r)] {
                    for line in text.lines().skip(2) {
                        assert!(
                            line.bytes()
                                .all(|b| b.is_ascii_alphanumeric()
                                    || b" @_.,=(){}[]#\"".contains(&b)),
                            "seed {seed}: {line:?}"
                        );
                        assert_eq!(line.matches('"').count() % 2, 0, "seed {seed}: {line:?}");
                    }
                }
                for line in velatura_theme::ptyxis::render(&r).lines().skip(2) {
                    let ok = line.is_empty()
                        || matches!(line, "[Palette]" | "Name=Velatura" | "[Light]" | "[Dark]")
                        || line
                            .split_once('=')
                            .is_some_and(|(_, value)| Color::parse(value).is_ok());
                    assert!(ok, "seed {seed}: {line:?}");
                }
                for line in velatura_theme::sourceview::render(&r).lines().skip(1) {
                    assert!(
                        line.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b" <>/!=\"#:.-".contains(&b)),
                        "seed {seed}: {line:?}"
                    );
                }
                for line in gtk::render(&r).lines().skip(2) {
                    assert!(
                        line.starts_with("@define-color ") && line.ends_with(';'),
                        "seed {seed}: {line:?}"
                    );
                    assert!(
                        line.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b" @-_#(),.;".contains(&b)),
                        "seed {seed}: {line:?}"
                    );
                }
            }
        }
    }
    // The mutations must be gentle enough that a good share still loads,
    // or the renderers would not be exercised at all.
    assert!(loaded > 1_500, "only {loaded} mutated themes loaded");
    eprintln!("{loaded} of 30000 mutated themes loaded and were rendered");
}

/// The same for files in the other formats that can be imported: damaged
/// copies never panic, and whatever is read is a theme the validator
/// passes, with an id that can be a file name.
#[test]
fn damaged_files_in_other_formats_never_panic() {
    let fixtures = [
        include_str!("fixtures/base16-ocean.yaml"),
        include_str!("fixtures/base16-ocean-palette.yaml"),
        include_str!("fixtures/base24-one-light.yaml"),
        include_str!("fixtures/kitty-dracula.conf"),
    ];
    let mut read = 0;
    for (n, source) in fixtures.iter().enumerate() {
        for seed in 1..=5_000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ n as u64);
            let bytes = mutate(source.as_bytes(), &mut rng);
            let text = String::from_utf8_lossy(&bytes);
            let Some(Ok(found)) = velatura_theme::import::read(&text, "a file") else {
                continue;
            };
            let loaded = &found.loaded;
            assert!(found.accent < found.accents.len(), "seed {seed}");
            for (_, accent) in &found.accents {
                assert!(
                    velatura_theme::import::with_accent(&loaded.theme, *accent).is_some(),
                    "seed {seed}"
                );
            }
            read += 1;
            let theme = &loaded.theme;
            assert!(velatura_theme::model::is_slug(&theme.id), "seed {seed}");
            let again = parse_theme(&velatura_theme::model::to_toml(theme));
            assert_eq!(
                again.map(|l| l.theme).ok().as_ref(),
                Some(theme),
                "seed {seed}"
            );
        }
    }
    assert!(read > 2_000, "only {read} damaged files were read");
}
