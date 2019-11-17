use crossbeam::epoch::{pin, Atomic};
use std::sync::atomic::Ordering;

#[derive(Debug)]
struct Canary {
    name: String,
}

impl Canary {
    fn new(name: &str) -> Canary {
        Canary {
            name: name.to_owned(),
        }
    }
}

impl Drop for Canary {
    fn drop(&mut self) {
        println!("{}: dropped", self.name);
    }
}

struct BirdCage {
    c: Atomic<Canary>,
}

fn cleanup(n: usize) {
    println!("[{}] doing deferred cleanup", n);
}

fn access(birdcage: &BirdCage, n: usize) {
    let guard = &pin();
    let shared = birdcage.c.load(Ordering::SeqCst, guard);
    let c: &Canary = unsafe{shared.as_ref()}.unwrap();
    println!("[{}] accessing {}", n, c.name);
    guard.defer(move || {
        cleanup(n);
    });

    // Uncomment this to see the deferred function run sooner.
    // Otherwise, the default Collector will wait until a bunch of
    // deferred actions have accumulated (~256 in crossbeam 0.7.3).

    //guard.flush();
}

fn main() {
    let birdcage = BirdCage {
        c: Atomic::new(Canary::new("first")),
    };
    // Increase this number to see the deferred functions run.
    for n in 0..10 {
        access(&birdcage, n);
    }
}
