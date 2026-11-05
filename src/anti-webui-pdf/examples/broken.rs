//! What `hayro` does with broken documents: `broken <rounds> <file.pdf>...` draws copies of each with
//! random damage, and says what panicked and what took long. See `docs/design/26-pdf.md`.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The size of xorshift, which is all this needs.
struct Random(u64);

impl Random {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below.max(1) as u64) as usize
    }
}

fn damage(data: &[u8], kind: usize, random: &mut Random) -> Vec<u8> {
    let mut out = data.to_vec();
    match kind {
        // Cut short, as a download that stopped.
        0 => out.truncate(random.next(data.len())),
        // Single bytes, which is what a bad disk does.
        1 => {
            for _ in 0..(data.len() / 2000).clamp(8, 400) {
                let at = random.next(out.len());
                out[at] = random.next(256) as u8;
            }
        }
        // A hole.
        _ => {
            let at = random.next(out.len());
            let end = (at + 1 + random.next(out.len() / 10 + 1)).min(out.len());
            out[at..end].fill(0);
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rounds: u64 = args[1].parse().unwrap();
    let panics = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    {
        let panics = panics.clone();
        std::panic::set_hook(Box::new(move |info| {
            let at = info.location().map_or("somewhere".to_owned(), |l| {
                format!("{}:{}", l.file(), l.line())
            });
            *panics.lock().unwrap().entry(at).or_default() += 1;
        }));
    }
    let pixmap = PixmapSettings {
        x_scale: 1.5,
        y_scale: 1.5,
        bg_color: WHITE,
    };
    let settings = RenderSettings::default();
    let interpreter = InterpreterSettings::default();
    let (mut opened, mut refused, mut drawn, mut panicked) = (0, 0, 0, 0);
    let mut slowest = (Duration::ZERO, String::new());
    for file in &args[2..] {
        let data = std::fs::read(file).unwrap();
        for round in 0..rounds {
            let mut random =
                Random(0x9E3779B97F4A7C15 ^ (round + 1).wrapping_mul(0xD1342543DE82EF95));
            let kind = round as usize % 3;
            let broken = damage(&data, kind, &mut random);
            let name = format!("{file} round {round} kind {kind}");
            let start = Instant::now();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let Ok(pdf) = Pdf::new(broken) else {
                    refused += 1;
                    return;
                };
                opened += 1;
                let cache = RenderCache::new();
                // A page in five is enough to see what comes of it, and the first is always one.
                for (i, page) in pdf.pages().iter().enumerate().take(50) {
                    if i % 5 != 0 {
                        continue;
                    }
                    let started = Instant::now();
                    let page_done = catch_unwind(AssertUnwindSafe(|| {
                        render(page, &cache, &interpreter, &settings, &pixmap);
                    }));
                    match page_done {
                        Ok(()) => drawn += 1,
                        Err(_) => {
                            panicked += 1;
                            eprintln!("panic in {name}, page {i}");
                        }
                    }
                    let took = started.elapsed();
                    if took > slowest.0 {
                        slowest = (took, format!("{name}, page {i}"));
                    }
                }
            }));
            if outcome.is_err() {
                panicked += 1;
                eprintln!("panic opening {name}");
            }
            let took = start.elapsed();
            if took > Duration::from_secs(20) {
                eprintln!("{name} took {took:?}");
            }
        }
    }
    println!(
        "opened {opened}, refused {refused}, pages drawn {drawn}, panics {panicked}, slowest page {:?} in {}",
        slowest.0, slowest.1
    );
    for (at, count) in panics.lock().unwrap().iter() {
        println!("{count} at {at}");
    }
}
