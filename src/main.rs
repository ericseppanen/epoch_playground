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

fn main() {
    let a = Atomic::new(Canary::new("first"));

    let guard = &pin();
    let shared = a.load(Ordering::SeqCst, guard);
    let c: &Canary = unsafe{shared.as_ref()}.unwrap();
    println!("accessing {}", c.name);
}
